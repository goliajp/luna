//! A `Cell` a memory policy can hold with the `send` feature, where a
//! policy is `Send`.

use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct SyncCell<T: Copy>(Mutex<T>);

impl<T: Copy> SyncCell<T> {
    pub const fn new(v: T) -> SyncCell<T> {
        SyncCell(Mutex::new(v))
    }

    pub fn get(&self) -> T {
        *self.0.lock().unwrap()
    }

    pub fn set(&self, v: T) {
        *self.0.lock().unwrap() = v;
    }
}
