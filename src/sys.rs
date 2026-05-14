use std::{
    io,
    process::{ChildStderr, ChildStdout},
};

// std supports unix, windows, uefi and motor.
// atm it is enough to support just unix and windows as it covers the majority of use cases.
#[cfg(target_family = "unix")]
use self::unix as imp;
#[cfg(target_os = "windows")]
use self::windows as imp;

/// Reads streams concurrently.
pub(crate) fn read_output(
    p1: ChildStdout,
    v1: &mut Vec<u8>,
    p2: ChildStderr,
    v2: &mut Vec<u8>,
) -> io::Result<()> {
    imp::read_output(p1, v1, p2, v2)
}

// implementation is mostly taken from stdlib.
#[cfg(target_family = "unix")]
mod unix {
    use std::{
        io::{self, Read},
        mem,
        os::fd::{AsRawFd, RawFd},
        process::{ChildStderr, ChildStdout},
    };

    pub(crate) fn read_output(
        mut p1: ChildStdout,
        v1: &mut Vec<u8>,
        mut p2: ChildStderr,
        v2: &mut Vec<u8>,
    ) -> io::Result<()> {
        set_nonblocking(p1.as_raw_fd(), true)?;
        set_nonblocking(p2.as_raw_fd(), true)?;

        let default_pollfd: libc::pollfd = unsafe { mem::zeroed() };
        let mut fds: [libc::pollfd; 2] = [
            libc::pollfd {
                fd: p1.as_raw_fd(),
                events: libc::POLLIN,
                ..default_pollfd
            },
            libc::pollfd {
                fd: p2.as_raw_fd(),
                events: libc::POLLIN,
                ..default_pollfd
            },
        ];

        loop {
            loop {
                let res = unsafe { cvt(libc::poll(fds.as_mut_ptr(), fds.len() as _, -1)) };
                if let Err(ref err) = res {
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                }
                res?;
                break;
            }

            if fds[0].revents != 0 && read(&mut p1, v1)? {
                set_nonblocking(p2.as_raw_fd(), false)?;
                return p2.read_to_end(v2).map(drop);
            }
            if fds[1].revents != 0 && read(&mut p2, v2)? {
                set_nonblocking(p1.as_raw_fd(), false)?;
                return p1.read_to_end(v1).map(drop);
            }
        }

        // Read as much as we can from each pipe, ignoring EWOULDBLOCK or
        // EAGAIN. If we hit EOF, then this will happen because the underlying
        // reader will return Ok(0), in which case we'll see `Ok` ourselves. In
        // this case we flip the other fd back into blocking mode and read
        // whatever's leftover on that file descriptor.
        fn read(fd: &mut impl Read, dst: &mut Vec<u8>) -> Result<bool, io::Error> {
            match fd.read_to_end(dst) {
                Ok(_) => Ok(true),
                Err(e) => {
                    if e.raw_os_error() == Some(libc::EWOULDBLOCK)
                        || e.raw_os_error() == Some(libc::EAGAIN)
                    {
                        Ok(false)
                    } else {
                        Err(e)
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn set_nonblocking(fd: RawFd, nonblocking: bool) -> io::Result<()> {
        unsafe {
            let v = nonblocking as libc::c_int;
            cvt(libc::ioctl(fd, libc::FIONBIO, &v))?;
            Ok(())
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn set_nonblocking(fd: RawFd, nonblocking: bool) -> io::Result<()> {
        unsafe {
            let prev = cvt(libc::fcntl(fd, libc::F_GETFL))?;
            let new = if nonblocking {
                prev | libc::O_NONBLOCK
            } else {
                prev & !libc::O_NONBLOCK
            };
            if new != prev {
                cvt(libc::fcntl(fd, libc::F_SETFL, new))?;
            }
            Ok(())
        }
    }

    /// Converts native return values to Result using the *-1 means error is in `errno`*  convention.
    /// Non-error values are `Ok`-wrapped.
    fn cvt(t: libc::c_int) -> io::Result<libc::c_int> {
        if t == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(t)
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        io, mem,
        os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle},
        process::{ChildStderr, ChildStdout},
    };

    use windows::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0},
        Storage::FileSystem::ReadFile,
        System::{
            IO::{CancelIo, GetOverlappedResult, OVERLAPPED},
            Threading::{CreateEventW, INFINITE, WaitForMultipleObjects},
        },
    };
    use windows_core::Owned;

    pub(crate) fn read_output(
        p1: ChildStdout,
        v1: &mut Vec<u8>,
        p2: ChildStderr,
        v2: &mut Vec<u8>,
    ) -> io::Result<()> {
        let p1 = p1.as_handle();
        let p2 = p2.as_handle();
        let mut p1 = AsyncPipe::new(p1, v1)?;
        let mut p2 = AsyncPipe::new(p2, v2)?;
        let objs: &[_] = &[p1.event(), p2.event()];

        loop {
            let res = unsafe { WaitForMultipleObjects(objs, false, INFINITE) };
            let (finished, other) = if res.0 == WAIT_OBJECT_0.0 {
                (&mut p1, &mut p2)
            } else if res.0 == WAIT_OBJECT_0.0 + 1 {
                (&mut p2, &mut p1)
            } else {
                return Err(io::Error::last_os_error());
            };
            if !finished.read_once()? {
                return other.finish();
            }
        }
    }

    struct AsyncPipe<'a> {
        pipe: BorrowedHandle<'a>,
        event: Owned<HANDLE>,
        overlapped: Box<OVERLAPPED>, // needs a stable address
        dst: &'a mut Vec<u8>,
        state: State,
    }

    #[derive(PartialEq, Debug)]
    enum State {
        NotReading,
        Reading,
        Read(usize),
    }

    impl<'a> Drop for AsyncPipe<'a> {
        fn drop(&mut self) {
            let State::Reading = self.state else { return };

            // If we have a pending read operation, then we have to make sure that
            // it's *done* before we actually drop this type. The kernel requires
            // that the `OVERLAPPED` and buffer pointers are valid for the entire
            // I/O operation.
            //
            // To do that, we call `CancelIo` to cancel any pending operation, and
            // if that succeeds we wait for the overlapped result.
            //
            // If anything here fails, there's not really much we can do, so we leak
            // the buffer/OVERLAPPED pointers to ensure we're at least memory safe.
            let res = unsafe { CancelIo(HANDLE(self.pipe.as_raw_handle())) };
            if res.is_err() || self.result().is_err() {
                let buf = mem::take(self.dst);
                let overlapped = mem::take(&mut self.overlapped);
                mem::forget((buf, overlapped));
            }
        }
    }

    impl<'a> AsyncPipe<'a> {
        fn new(pipe: BorrowedHandle<'a>, dst: &'a mut Vec<u8>) -> io::Result<AsyncPipe<'a>> {
            // Create an event which we'll use to coordinate our overlapped
            // operations, this event will be used in WaitForMultipleObjects
            // and passed as part of the OVERLAPPED handle.
            //
            // Note that we do a somewhat clever thing here by flagging the
            // event as being manually reset and setting it initially to the
            // signaled state. This means that we'll naturally fall through the
            // WaitForMultipleObjects call above for pipes created initially,
            // and the only time an even will go back to "unset" will be once an
            // I/O operation is successfully scheduled (what we want).
            let event = unsafe { Owned::new(CreateEventW(None, true, true, None)?) };
            let mut overlapped: Box<OVERLAPPED> = Box::new(Default::default());
            overlapped.hEvent = *event;
            Ok(AsyncPipe {
                pipe,
                overlapped,
                event,
                dst,
                state: State::NotReading,
            })
        }

        fn event(&self) -> HANDLE {
            *self.event
        }

        /// Executes an overlapped read operation.
        ///
        /// Must not currently be reading, and returns whether the pipe is currently
        /// at EOF or not. If the pipe is not at EOF then `result()` must be called
        /// to complete the read later on (may block), but if the pipe is at EOF
        /// then `result()` should not be called as it will just block forever.
        fn schedule_read(&mut self) -> io::Result<bool> {
            assert_eq!(self.state, State::NotReading);
            if self.dst.capacity() == self.dst.len() {
                let additional = if self.dst.capacity() == 0 { 16 } else { 1 };
                self.dst.reserve(additional);
            }
            let amt = unsafe {
                let dst = self.dst.spare_capacity_mut();
                let dst = &mut *std::ptr::slice_from_raw_parts_mut(
                    dst.as_mut_ptr().cast::<u8>(),
                    dst.len(),
                );
                let mut amt = 0;
                let res = ReadFile(
                    HANDLE(self.pipe.as_raw_handle()),
                    Some(dst),
                    Some(&mut amt),
                    Some(&mut *self.overlapped),
                );
                match res {
                    Ok(()) => Some(amt as usize),
                    Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => None,
                    Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Some(0),
                    Err(e) => return Err(e.into()),
                }
            };

            // If this read finished immediately then our overlapped event will
            // remain signaled (it was signaled coming in here) and we'll progress
            // down to the method below.
            //
            // Otherwise the I/O operation is scheduled and the system set our event
            // to not signaled, so we flag ourselves into the reading state and move
            // on.
            self.state = match amt {
                Some(0) => return Ok(false),
                Some(amt) => State::Read(amt),
                None => State::Reading,
            };
            Ok(true)
        }

        /// Wait for the result of the overlapped operation previously executed.
        ///
        /// Takes a parameter `wait` which indicates if this pipe is currently being
        /// read whether the function should block waiting for the read to complete.
        ///
        /// Returns values:
        ///
        /// * `true` - finished any pending read and the pipe is not at EOF (keep
        ///            going)
        /// * `false` - finished any pending read and pipe is at EOF (stop issuing
        ///             reads)
        fn result(&mut self) -> io::Result<bool> {
            let amt = match self.state {
                State::NotReading => return Ok(true),
                State::Reading => {
                    let mut amt = 0;
                    let res = unsafe {
                        GetOverlappedResult(
                            HANDLE(self.pipe.as_raw_handle()),
                            &*self.overlapped,
                            &mut amt,
                            true,
                        )
                    };
                    match res {
                        Ok(()) => amt as usize,
                        Err(e)
                            if e.code() == ERROR_IO_PENDING.to_hresult()
                                || e.code() == ERROR_BROKEN_PIPE.to_hresult() =>
                        {
                            0
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                State::Read(amt) => amt,
            };
            self.state = State::NotReading;
            unsafe {
                let len = self.dst.len();
                self.dst.set_len(len + amt);
            }
            Ok(amt != 0)
        }

        fn read_once(&mut self) -> io::Result<bool> {
            Ok(self.result()? && self.schedule_read()?)
        }

        /// Finishes out reading this pipe entirely.
        ///
        /// Waits for any pending and schedule read, and then calls `read_to_end`
        /// if necessary to read all the remaining information.
        fn finish(&mut self) -> io::Result<()> {
            while self.read_once()? {
                // ...
            }
            Ok(())
        }
    }
}
