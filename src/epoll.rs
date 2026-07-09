use std::io;
use std::os::fd::RawFd;

use crate::ffi::{
    close_fd, create_epoll, ctl, wait, EpollEvent, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD,
};

pub struct Epoll {
    fd: RawFd,
}

impl Epoll {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            fd: create_epoll()?,
        })
    }

    pub fn add(&self, fd: RawFd, events: u32) -> io::Result<()> {
        let mut event = EpollEvent::new(events, fd as u64);
        ctl(self.fd, EPOLL_CTL_ADD, fd, Some(&mut event))
    }

    pub fn delete(&self, fd: RawFd) {
        let _ = ctl(self.fd, EPOLL_CTL_DEL, fd, None);
    }

    pub fn modify(&self, fd: RawFd, events: u32) -> io::Result<()> {
        let mut event = EpollEvent::new(events, fd as u64);
        ctl(self.fd, EPOLL_CTL_MOD, fd, Some(&mut event))
    }

    pub fn wait(&self, events: &mut [EpollEvent], timeout_ms: i32) -> io::Result<usize> {
        wait(self.fd, events, timeout_ms)
    }
}

impl Drop for Epoll {
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}
