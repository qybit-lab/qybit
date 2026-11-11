//! 中断与终止信号（仅 Linux）：Ctrl+C（SIGINT）、终止请求（SIGTERM：`kill`、服务管理器停止、
//! 关机）与终端挂断（SIGHUP：关闭终端窗口、SSH 断开）。第一次收到其中任何一个时只置位中断
//! 标志，由播放驱动停止设备、恢复设备硬件音量（A4）并给出完整报告；处理函数同时把这三个信号都
//! 恢复为默认处理，再收到任何一个即按默认方式结束进程，保证总能退出。
//!
//! 不处理终止信号的后果：进程被默认处理直接结束，播放期间调整过的设备硬件音量停在调整后的值，
//! 违反“播放结束恢复原值”（AGENTS §3）。SIGKILL 无法捕获，来不及恢复。
//!
//! 出处：`signal(2)`、`signal(7)`（三个信号的默认动作都是结束进程）、`signal-safety(7)`（处理
//! 函数中只做原子写与 `signal` 调用，二者都是异步信号安全的）。
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// 按“停止播放并收尾”处理的信号。
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

extern "C" fn handler(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Release);
    for sig in SIGNALS {
        // SAFETY: signal 是异步信号安全函数；恢复默认处理。
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
        }
    }
}

/// 安装处理函数，返回中断标志。
pub fn install() -> Result<&'static AtomicBool, String> {
    let f: extern "C" fn(libc::c_int) = handler;
    for sig in SIGNALS {
        // SAFETY: handler 只做原子写与 signal 调用（异步信号安全）。
        let prev = unsafe { libc::signal(sig, f as libc::sighandler_t) };
        if prev == libc::SIG_ERR {
            return Err(format!(
                "安装信号 {sig} 的处理失败：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(&INTERRUPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 终止请求（SIGTERM）与 Ctrl+C 一样只置位标志、进程继续运行（由播放驱动收尾并恢复硬件
    /// 音量）；之后三个信号都已恢复为默认处理，再来一个即结束进程。
    #[test]
    fn termination_request_sets_the_flag_and_restores_defaults() {
        let flag = match install() {
            Ok(f) => f,
            Err(e) => panic!("{e}"),
        };
        assert!(!flag.load(Ordering::Acquire));
        // SAFETY: 向本线程发送一个已安装处理函数的信号；处理函数只做原子写与 signal 调用。
        assert_eq!(unsafe { libc::raise(libc::SIGTERM) }, 0);
        assert!(flag.load(Ordering::Acquire), "收到 SIGTERM 后应置位");
        for sig in SIGNALS {
            // SAFETY: 读取并保持默认处理（设置为默认，返回此前的处理方式）。
            let prev = unsafe { libc::signal(sig, libc::SIG_DFL) };
            assert_eq!(prev, libc::SIG_DFL, "信号 {sig} 应已恢复默认处理");
        }
    }
}
