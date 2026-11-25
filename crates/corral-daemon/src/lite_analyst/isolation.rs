//! Linux fixture process and network boundary. The trusted runner retains the staged
//! runtime and outer namespaces. Each payload sees only its own executable and ABI.
use anyhow::{Context, Result, ensure};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::{
    fs::{self, File},
    io::{Seek, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const BWRAP: &str = "/usr/bin/bwrap";
const SCRATCH: &str = "/scratch";
const PACKAGE: &str = "/fixture/corral";

pub(super) fn protect_supervisor() -> Result<()> {
    use std::io::Read;
    let mut interfaces = String::new();
    File::open("/proc/net/dev")
        .context("open fixture interfaces")?
        .take(8193)
        .read_to_string(&mut interfaces)
        .context("read fixture interfaces")?;
    ensure!(
        interfaces.len() <= 8192
            && interfaces
                .lines()
                .skip(2)
                .map(|line| line.split_once(':').map(|(name, _)| name.trim()))
                .eq([Some("lo")]),
        "fixture requires a dedicated loopback-only namespace"
    );
    ensure!(
        !Path::new("/.git").exists() && !Path::new("/scratch/.git").exists(),
        "fixture scratch must be outside Git"
    );
    let meta = fs::symlink_metadata(BWRAP).context("inspect fixture sandbox launcher")?;
    ensure!(
        meta.is_file() && meta.mode() & 0o6000 == 0 && meta.mode() & 0o111 != 0,
        "fixture requires a non-setid sandbox launcher"
    );
    install_filter(&socket_filter(true))
}

fn install_filter(filter: &[libc::sock_filter]) -> Result<()> {
    let program = libc::sock_fprog {
        len: filter.len().try_into().context("fixture BPF size")?,
        filter: filter.as_ptr().cast_mut(),
    };
    // SAFETY: immutable live BPF instructions and program are copied by seccomp;
    // Bootstrap already required no_new_privs on this fresh thread.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0_u32,
            &program,
        )
    };
    ensure!(rc == 0, "install fixture socket filter");
    Ok(())
}

pub(super) enum Payload {
    Shim,
    Pi,
}

impl Payload {
    fn entrypoint(&self) -> &'static str {
        match self {
            Self::Shim => "/fixture/corral/bin/corral-lite-mcp-shim",
            Self::Pi => "/fixture/corral/bin/corral-pi-lite-analyst",
        }
    }
}

/// Owns only the waitable bwrap child created here, never a looked-up PID. A
/// normal wrapper exit has reaped its private namespace init. A forced or
/// otherwise unproven exit cannot release the durable slot.
pub(super) struct Sandbox(Child);

impl Sandbox {
    pub(super) fn spawn(payload: Payload) -> Result<(Self, UnixStream)> {
        let filter = payload_filter()?;
        let filter_fd = filter.as_raw_fd();
        let (host, child) = UnixStream::pair().context("create fixture stdio pair")?;
        let child_in: OwnedFd = child.try_clone().context("clone fixture stdin")?.into();
        let child_out: OwnedFd = child.into();
        host.set_nonblocking(true)
            .context("configure fixture host stdio")?;
        let entry = payload.entrypoint();
        let mut cmd = Command::new(BWRAP);
        cmd.env_clear()
            .current_dir(SCRATCH)
            .stdin(Stdio::from(child_in))
            .stdout(Stdio::from(child_out))
            .stderr(Stdio::null())
            .args([
                "--unshare-all",
                "--unshare-user",
                "--disable-userns",
                "--new-session",
                "--die-with-parent",
                "--cap-drop",
                "ALL",
                "--clearenv",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--size",
                "8388608",
                "--perms",
                "0700",
                "--tmpfs",
                SCRATCH,
                "--dir",
                "/scratch/home",
                "--dir",
                "/scratch/tmp",
                "--dir",
                "/scratch/config",
                "--dir",
                "/scratch/data",
                "--dir",
                "/scratch/cache",
                "--dir",
                "/scratch/state",
                "--dir",
                "/scratch/run",
            ])
            .args([
                "--ro-bind",
                &format!("{PACKAGE}/runtime/lib"),
                "/lib",
                "--ro-bind",
                &format!("{PACKAGE}/runtime/lib64"),
                "/lib64",
                "--ro-bind",
                entry,
                entry,
                "--remount-ro",
                "/",
                "--chdir",
                SCRATCH,
                "--seccomp",
                &filter_fd.to_string(),
            ]);
        if matches!(payload, Payload::Pi) {
            for (key, value) in [
                ("LC_ALL", "C.UTF-8"),
                ("PATH", "/nonexistent"),
                ("HOME", "/scratch/home"),
                ("TMPDIR", "/scratch/tmp"),
                ("XDG_CONFIG_HOME", "/scratch/config"),
                ("XDG_DATA_HOME", "/scratch/data"),
                ("XDG_CACHE_HOME", "/scratch/cache"),
                ("XDG_STATE_HOME", "/scratch/state"),
                ("XDG_RUNTIME_DIR", "/scratch/run"),
            ] {
                cmd.args(["--setenv", key, value]);
            }
        }
        cmd.args(["--", entry]);
        // SAFETY: only async-signal-safe libc calls, no allocations or locks in
        // the successful path. Preserve Command's exec-error pipe until exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::syscall(
                    libc::SYS_close_range,
                    3_u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) != 0
                    || libc::fcntl(filter_fd, libc::F_SETFD, 0) != 0
                    || libc::setrlimit(
                        libc::RLIMIT_CORE,
                        &libc::rlimit {
                            rlim_cur: 0,
                            rlim_max: 0,
                        },
                    ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let running = cmd.spawn().context("launch isolated fixture payload")?;
        Ok((Self(running), host))
    }

    /// Caller has first closed the supervised stdio/control endpoint. A hung
    /// wrapper is killed by retained child identity; we still quarantine the
    /// slot because a killed wrapper alone is not proof of descendant teardown.
    pub(super) fn reap(&mut self, deadline: Instant) -> Result<()> {
        use std::os::unix::process::ExitStatusExt;
        loop {
            if let Some(status) = self.0.try_wait().context("observe fixture wrapper")? {
                // bubblewrap can mirror the sandbox exit status. A signal that
                // directly removed the wrapper cannot be distinguished here.
                ensure!(
                    status.signal().is_none(),
                    "fixture wrapper teardown unproved"
                );
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.0.kill().context("kill retained fixture wrapper")?;
                // Reap the wrapper if the kernel returns it, but do not promote
                // the forced wrapper exit into proof that its init was reaped.
                let force_deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < force_deadline
                    && self
                        .0
                        .try_wait()
                        .context("reap killed fixture wrapper")?
                        .is_none()
                {
                    thread::sleep(Duration::from_millis(20));
                }
                anyhow::bail!("fixture wrapper teardown unproved");
            }
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }
}

fn payload_filter() -> Result<File> {
    let fd = memfd_create(
        "fixture-payload-seccomp",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .context("create fixture syscall filter")?;
    let mut file = File::from(fd);
    for op in socket_filter(false) {
        file.write_all(&op.code.to_ne_bytes())
            .context("write fixture syscall filter")?;
        file.write_all(&[op.jt, op.jf])
            .context("write fixture syscall filter branches")?;
        file.write_all(&op.k.to_ne_bytes())
            .context("write fixture syscall filter operand")?;
    }
    file.rewind().context("rewind fixture syscall filter")?;
    fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .context("seal fixture syscall filter")?;
    Ok(file)
}

// Classic BPF on seccomp_data, Linux x86-64 only. The supervisor can create
// unnamed Unix pairs and IPv4 TCP streams, with the TCP port under Landlock.
// Also admit bwrap's local loopback provisioning sockets; deny them, and all
// other socket creation, once the payload starts.
fn socket_filter(supervisor: bool) -> Vec<libc::sock_filter> {
    fn ins(code: u16, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }
    fn branch(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }
    const LD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    const AND: u16 = (libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16;
    const EQ: u16 = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    const GE: u16 = (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16;
    const RET: u16 = (libc::BPF_RET | libc::BPF_K) as u16;
    const DENY: u32 = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    const ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
    let mut ops = vec![
        ins(LD, 4),                   // arch
        branch(EQ, 0xc000003e, 1, 0), // AUDIT_ARCH_X86_64
        ins(RET, libc::SECCOMP_RET_KILL_PROCESS),
        ins(LD, 0),                    // syscall number
        branch(GE, 0x4000_0000, 0, 1), // reject x32 syscall ABI
        ins(RET, libc::SECCOMP_RET_KILL_PROCESS),
    ];
    let mut deny_syscall = |nr| ops.extend([branch(EQ, nr as u32, 0, 1), ins(RET, DENY)]);
    for nr in [
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ] {
        deny_syscall(nr);
    }
    if supervisor {
        ops.extend([
            branch(EQ, libc::SYS_socketpair as u32, 0, 4),
            ins(LD, 16), // args[0]
            branch(EQ, libc::AF_UNIX as u32, 0, 1),
            ins(RET, ALLOW),
            ins(RET, DENY),
        ]);
        let socket_rules = [
            (libc::AF_INET, libc::SOCK_STREAM, libc::IPPROTO_TCP),
            (libc::AF_INET, libc::SOCK_STREAM, 0),
            // bwrap uses SIOCGIFINDEX and NETLINK_ROUTE in its new netns.
            // Neither grants Internet UDP/raw traffic or an additional TCP route.
            (libc::AF_UNIX, libc::SOCK_DGRAM, 0),
            (libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE),
        ];
        ops.push(branch(
            EQ,
            libc::SYS_socket as u32,
            0,
            (8 * socket_rules.len() + 1) as u8,
        ));
        for (domain, kind, protocol) in socket_rules {
            ops.extend([
                ins(LD, 16), // args[0]
                branch(EQ, domain as u32, 0, 6),
                ins(LD, 24), // args[1]
                ins(AND, !(libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK) as u32),
                branch(EQ, kind as u32, 0, 3),
                ins(LD, 32), // args[2]
                branch(EQ, protocol as u32, 0, 1),
                ins(RET, ALLOW),
            ]);
        }
        ops.push(ins(RET, DENY));
    } else {
        for nr in [
            libc::SYS_socket,
            libc::SYS_socketpair,
            libc::SYS_setns,
            libc::SYS_unshare,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
        ] {
            deny_syscall(nr);
        }
    }
    ops.push(ins(RET, ALLOW));
    ops
}

#[cfg(test)]
#[path = "isolation_tests.rs"]
mod tests;
