//! Linux fixture process and network boundary. The trusted runner retains the staged
//! runtime and outer namespaces. Each payload sees only its own executable and ABI.
use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{MemfdFlags, OFlags, SealFlags, fcntl_add_seals, fcntl_setfl, memfd_create},
    io::Errno,
    net::{SendFlags, send},
    pipe::{PipeFlags, pipe_with},
    process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal},
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Seek, Write},
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
const INFO_MAX: usize = 4096;
const STEP: Duration = Duration::from_millis(20);
const FORCE_GRACE: Duration = Duration::from_secs(1);

pub(super) fn protect_supervisor() -> Result<()> {
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

/// Run on the actual supervisor thread after loopback-only namespace verification,
/// seccomp and Landlock. Even if the restriction is broken, send no data and
/// never connect off loopback or to the provider port.
#[cfg(feature = "lite-analyst-faults")]
pub(super) fn probe_supervisor_denial() -> Result<()> {
    use rustix::net::{self, AddressFamily, SocketFlags, SocketType, ipproto};
    use std::net::{Ipv4Addr, SocketAddrV4};

    let flags = SocketFlags::CLOEXEC | SocketFlags::NONBLOCK;
    let tcp = net::socket_with(
        AddressFamily::INET,
        SocketType::STREAM,
        flags,
        Some(ipproto::TCP),
    )
    .context("open fixture supervisor probe socket")?;
    let unused = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 43174);
    ensure!(
        net::connect(&tcp, &unused) == Err(Errno::ACCESS),
        "fixture supervisor probe did not observe Landlock TCP denial"
    );
    ensure!(
        matches!(
            net::socket_with(
                AddressFamily::INET,
                SocketType::DGRAM,
                flags,
                Some(ipproto::UDP),
            ),
            Err(Errno::PERM)
        ),
        "fixture supervisor probe did not observe seccomp UDP socket denial"
    );
    Ok(())
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
    #[cfg(feature = "lite-analyst-faults")]
    PiProbe,
}

impl Payload {
    fn is_pi(&self) -> bool {
        match self {
            Self::Shim => false,
            Self::Pi => true,
            #[cfg(feature = "lite-analyst-faults")]
            Self::PiProbe => true,
        }
    }

    fn entrypoint(&self) -> &'static str {
        if self.is_pi() {
            "/fixture/corral/bin/corral-pi-lite-analyst"
        } else {
            "/fixture/corral/bin/corral-lite-mcp-shim"
        }
    }
}

/// The wrapper can report the payload's status while namespace PID 1 still
/// has descendants. Keep its own waitable Child and a separate, verified pidfd
/// for PID 1; neither identity alone proves teardown.
pub(super) struct Sandbox {
    wrapper: Child,
    init: Option<OwnedFd>,
    gate: Option<UnixStream>,
    info: InfoPipe,
}

impl Sandbox {
    pub(super) fn spawn(payload: Payload) -> Result<(Self, UnixStream)> {
        let filter = payload_filter()?;
        let (info_reader, info_writer) =
            pipe_with(PipeFlags::CLOEXEC).context("create bwrap info pipe")?;
        fcntl_setfl(&info_reader, OFlags::NONBLOCK).context("configure bwrap info reader")?;
        let (gate, gate_reader) = UnixStream::pair().context("create bwrap startup gate")?;
        // bwrap treats EOF on --block-fd as release. Pass a duplicate of our
        // peer endpoint as --sync-fd so namespace init itself prevents EOF if
        // we drop the gate or the wrapper dies. Without --as-pid-1, bwrap
        // closes the sync fd in the payload before exec.
        let inherited = [
            filter.as_raw_fd(),
            info_writer.as_raw_fd(),
            gate_reader.as_raw_fd(),
            gate.as_raw_fd(),
        ];
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
                &inherited[0].to_string(),
                "--info-fd",
                &inherited[1].to_string(),
                "--block-fd",
                &inherited[2].to_string(),
                "--sync-fd",
                &inherited[3].to_string(),
            ]);
        if payload.is_pi() {
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
        #[cfg(feature = "lite-analyst-faults")]
        if matches!(payload, Payload::PiProbe) {
            cmd.arg("--probe-sandbox");
        }
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
                    || inherited
                        .into_iter()
                        .any(|fd| libc::fcntl(fd, libc::F_SETFD, 0) != 0)
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
        let wrapper = cmd.spawn().context("launch isolated fixture payload")?;
        Ok((
            Self {
                wrapper,
                init: None,
                gate: Some(gate),
                info: InfoPipe {
                    reader: info_reader.into(),
                    raw: Vec::new(),
                },
            },
            host,
        ))
    }

    /// Store this Sandbox before calling; failure still needs `reap`. Do not
    /// use the returned stdio until the init identity is retained and released.
    pub(super) fn confirm_identity(
        &mut self,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let gate = self
            .gate
            .as_ref()
            .context("fixture gate already released")?;
        self.init = Some(retain_init(
            &mut self.wrapper,
            &mut self.info,
            deadline,
            pump,
        )?);
        pump()?;
        ensure!(
            Instant::now() < deadline,
            "fixture startup deadline elapsed"
        );
        ensure!(
            self.wrapper
                .try_wait()
                .context("observe fixture wrapper")?
                .is_none(),
            "fixture wrapper exited before startup release"
        );
        ensure!(
            send(gate, b"x", SendFlags::DONTWAIT | SendFlags::NOSIGNAL)
                .context("release fixture startup gate")?
                == 1,
            "fixture startup gate not released"
        );
        self.gate = None;
        Ok(())
    }

    /// Caller has first closed the supervised stdio/control endpoint. An init
    /// pidfd becomes readable only once Linux has finished exiting PID 1,
    /// which includes killing/reaping any remaining namespace descendants.
    pub(super) fn reap(&mut self, deadline: Instant) -> Result<()> {
        if self.init.is_none() && Instant::now() < deadline {
            // Cancellation may have interrupted the info read. Try once more
            // for cleanup only: never release the gate from this path.
            self.init =
                retain_init(&mut self.wrapper, &mut self.info, deadline, &mut || Ok(())).ok();
        }
        if self.init.is_some() && self.wait_teardown(deadline) {
            return Ok(());
        }
        if let Some(init) = &self.init {
            let _ = pidfd_send_signal(init, Signal::KILL);
        }
        let _ = self.wrapper.kill();
        ensure!(
            self.wait_teardown(Instant::now() + FORCE_GRACE),
            "fixture namespace and wrapper teardown unproved"
        );
        Ok(())
    }

    fn wait_teardown(&mut self, deadline: Instant) -> bool {
        loop {
            let wrapper_gone = self.wrapper.try_wait().is_ok_and(|status| status.is_some());
            if wrapper_gone {
                match self.init.as_ref().map(pidfd_exited) {
                    Some(Ok(true)) => return true,
                    None => return false,
                    _ => {}
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            thread::sleep(remaining.min(STEP));
        }
    }
}

struct InfoPipe {
    reader: File,
    raw: Vec<u8>,
}

impl InfoPipe {
    fn read(
        &mut self,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<BwrapInfo> {
        let mut chunk = [0; 1024];
        loop {
            pump()?;
            let remaining = deadline.saturating_duration_since(Instant::now()).min(STEP);
            ensure!(!remaining.is_zero(), "fixture startup deadline elapsed");
            match self.reader.read(&mut chunk) {
                Ok(0) => return serde_json::from_slice(&self.raw).context("decode bwrap info"),
                Ok(n) => {
                    self.raw.extend_from_slice(&chunk[..n]);
                    ensure!(self.raw.len() <= INFO_MAX, "bwrap info too large");
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    let mut fds = [PollFd::new(&self.reader, PollFlags::IN)];
                    let timeout = Timespec {
                        tv_sec: 0,
                        tv_nsec: remaining.subsec_nanos().into(),
                    };
                    match poll(&mut fds, Some(&timeout)) {
                        Ok(_) | Err(Errno::INTR) => {}
                        Err(e) => return Err(e).context("poll bwrap info"),
                    }
                }
                Err(e) => return Err(e).context("read bwrap info"),
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct BwrapInfo {
    child_pid: i32,
    pid_namespace: u64,
    mnt_namespace: u64,
    net_namespace: u64,
}

fn retain_init(
    wrapper: &mut Child,
    info: &mut InfoPipe,
    deadline: Instant,
    pump: &mut dyn FnMut() -> Result<()>,
) -> Result<OwnedFd> {
    let info = info.read(deadline, pump)?;
    ensure!(
        wrapper
            .try_wait()
            .context("observe fixture wrapper")?
            .is_none(),
        "fixture wrapper exited before init verification"
    );
    let parent = wrapper.id();
    let child = info.child_pid;
    ensure!(
        child > 1 && child as u32 != parent,
        "invalid bwrap child PID"
    );
    let pidfd = pidfd_open(
        Pid::from_raw(child).context("missing bwrap child PID")?,
        PidfdFlags::empty(),
    )
    .context("open fixture namespace init pidfd")?;
    let proc = format!("/proc/{child}");
    let status = fs::read_to_string(format!("{proc}/status")).context("read bwrap init status")?;
    ensure!(
        is_direct_init(&status, parent, child),
        "bwrap child is not its direct namespace init"
    );
    for (ns, reported) in [
        ("pid", info.pid_namespace),
        ("mnt", info.mnt_namespace),
        ("net", info.net_namespace),
    ] {
        let actual = fs::metadata(format!("{proc}/ns/{ns}")).context("inspect bwrap namespace")?;
        let outer =
            fs::metadata(format!("/proc/self/ns/{ns}")).context("inspect outer namespace")?;
        ensure!(
            actual.ino() == reported && reported != outer.ino(),
            "bwrap namespace not verified private"
        );
    }
    // A live pidfd after the procfs checks excludes PID reuse between opening
    // it and inspecting /proc. The un-reaped wrapper PID cannot be reused, and
    // only its trusted, still-gated clone can be its direct child and PID 1.
    ensure!(
        !pidfd_exited(&pidfd)?,
        "fixture init exited during verification"
    );
    Ok(pidfd)
}

fn is_direct_init(status: &str, wrapper: u32, child: i32) -> bool {
    let field = |key| status.lines().find_map(|line| line.strip_prefix(key));
    field("PPid:").and_then(|value| value.trim().parse::<u32>().ok()) == Some(wrapper)
        && field("NSpid:").is_some_and(|ids| {
            ids.split_ascii_whitespace()
                .map(str::parse::<i32>)
                .eq([Ok(child), Ok(1)])
        })
}

fn pidfd_exited(pidfd: &OwnedFd) -> Result<bool> {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    poll(&mut fds, Some(&Timespec::default())).context("poll fixture init pidfd")?;
    Ok(fds[0].revents().contains(PollFlags::IN))
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
