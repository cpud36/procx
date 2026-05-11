use std::{collections::BTreeMap, ffi::{OsStr, OsString}};


#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    clear: bool,
    vars: BTreeMap<OsString, Option<OsString>>,
}

impl Env {
    pub(crate) fn set(&mut self, key: &OsStr, value: &OsStr) {
        self.vars.insert(key.to_os_string(), Some(value.to_os_string()));
    }

    pub(crate) fn remove(&mut self, key: &OsStr) {
        if self.clear {
            self.vars.remove(key);
        } else {
            self.vars.insert(key.to_os_string(), None);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.clear = true;
        self.vars.clear();
    }

    pub(crate) fn configure(&self, cmd: &mut std::process::Command) {
        if self.clear {
            cmd.env_clear();
        }
        for (key, value) in self.vars.iter() {
            if let Some(value) = value {
                cmd.env(key, value);
            } else {
                cmd.env_remove(key);
            }
        }
    }
}