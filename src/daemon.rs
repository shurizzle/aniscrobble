use std::{io::Result, process::exit};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Whoami {
    Child,
    Parent(bool),
}

#[inline(always)]
fn check_err(result: i32) -> Result<i32> {
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub unsafe fn daemonize() -> Result<Whoami> {
    match check_err(unsafe { libc::fork() })? {
        0 => {
            if unsafe { libc::setsid() } == -1 {
                exit(1);
            }
            match unsafe { libc::fork() } {
                -1 => exit(1),
                0 => {
                    let null = check_err(unsafe {
                        libc::open(c"/dev/null".as_ptr().cast(), libc::O_RDWR)
                    })?;
                    for fd in 0..=2 {
                        check_err(unsafe { libc::dup2(null, fd) })?;
                    }
                    check_err(unsafe { libc::close(null) })?;
                    Ok(Whoami::Child)
                }
                _ => exit(0),
            }
        }
        pid => {
            let mut child_ret = 0;
            check_err(unsafe { libc::waitpid(pid, &mut child_ret, 0) })?;
            Ok(Whoami::Parent(child_ret == 0))
        }
    }
}
