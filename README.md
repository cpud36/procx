A convenient wrapper around `std::process::Command` that allows to construct commands as if they were just shell strings.
This library tries to be convenient to use in both one-off scripts and in larger projects.
It tries to do the right thing by default, but also provides api for more compilcated use cases.

# Examples

Getting a commit hash of a branch:
```rust
fn commit_hash(branch: &str) -> Result<String, xproc::Error> {
    xproc::cmd!("git rev-parse {branch}").read()
}
```

# See also

This crate is inspired by [`xshell`](https://crates.io/crates/xshell),
but aims for running larger command lines while keeping the ergonomics of the shell.