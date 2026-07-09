use std::io;
use std::os::fd::RawFd;
use std::os::raw::c_int;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;

pub const EPOLL_CTL_ADD: c_int = 1;
pub const EPOLL_CTL_DEL: c_int = 2;
pub const EPOLL_CTL_MOD: c_int = 3;
pub const EPOLL_CLOEXEC: c_int = 0x80000;

const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const O_NONBLOCK: c_int = 0o4000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

impl EpollEvent {
    pub fn new(events: u32, data: u64) -> Self {
        Self { events, data }
    }

    pub fn empty() -> Self {
        Self { events: 0, data: 0 }
    }
}

extern "C" {
    fn epoll_create1(flags: c_int) -> c_int;
    fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut EpollEvent) -> c_int;
    fn epoll_wait(epfd: c_int, events: *mut EpollEvent, maxevents: c_int, timeout: c_int) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
}

pub fn create_epoll() -> io::Result<RawFd> {
    let fd = unsafe { epoll_create1(EPOLL_CLOEXEC) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

pub fn ctl(
    epoll_fd: RawFd,
    op: c_int,
    fd: RawFd,
    event: Option<&mut EpollEvent>,
) -> io::Result<()> {
    let event_ptr = event
        .map(|event| event as *mut EpollEvent)
        .unwrap_or(std::ptr::null_mut());
    let result = unsafe { epoll_ctl(epoll_fd, op, fd, event_ptr) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn wait(epoll_fd: RawFd, events: &mut [EpollEvent], timeout_ms: i32) -> io::Result<usize> {
    loop {
        let result = unsafe {
            epoll_wait(
                epoll_fd,
                events.as_mut_ptr(),
                events.len() as c_int,
                timeout_ms,
            )
        };
        if result >= 0 {
            return Ok(result as usize);
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn close_fd(fd: RawFd) {
    let _ = unsafe { close(fd) };
}
