//! Local launch boundary. Descriptor custody and the full isolation/package proof belong
//! to the trusted launcher; neither a wire identity nor a matching UID proves custody.
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, stderr, stdin, stdout};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{self, FileType, Mode, OFlags};
use rustix::io::{self, Errno, FdFlags};
use rustix::net::{self, AddressFamily, SocketType, sockopt};
use serde::Deserialize;
use serde_json::{Value, json};

use super::wire::{
    POLICY_REF, PROMPT_SHA256, TOOL_POLICY_SHA256, decode, sha256, valid_digest, valid_id,
};

const PROFILE_PATH: &str = "/fixture/meta/launcher-v1.json";
const MANIFEST_PATH: &str = "/fixture/meta/build-manifest.json";
// Independent release pin, never learned from the wire, enrollment or manifest.
const PROFILE_SHA256: &str = "c145f94492ad6683432aeb293b96be3385c1b0f7f603dfdb6e81f29de069388c";
// Raw asset custody digest, distinct from the canonical digest in the policy ref.
const RAW_TOOL_POLICY_SHA256: &str =
    "38fae11185d2b48302d026ed788b04f1e5e6340fced8aa08f9d24707f9607de5";
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(15);
const PIPE_MAX: usize = 4096;
const CREDENTIAL_LEN: usize = 64;
const PROFILE_MAX: usize = 64 * 1024;
const MANIFEST_MAX: usize = 16 * 1024 * 1024;
const PIPEFS_MAGIC: fs::FsWord = 0x50495045; // Linux <linux/magic.h>

pub(super) struct Bootstrap {
    #[cfg(feature = "lite-analyst-faults")]
    pub(super) faults: Option<super::faults::FaultControl>,
    pub(super) enrollment: Enrollment,
    pub(super) work: UnixStream,
    pub(super) lifetime: OwnedFd,
    pub(super) resources: Resources,
}

pub(super) struct Enrollment {
    pub(super) link_id: String,
    pub(super) lite_instance: String,
    pub(super) corral_instance: String,
    pub(super) channel_epoch: String,
    pub(super) prior_channel_epoch: Option<String>,
    pub(super) profile_sha256: String,
    pub(super) corral_manifest_sha256: String,
    pub(super) joint_lock_sha256: String,
}

impl Enrollment {
    fn link_ready_for(&self, role: &str) -> Value {
        json!({"v": 1, "type": "link_ready", "role": role,
            "link_id": self.link_id, "lite_instance": self.lite_instance,
            "corral_instance": self.corral_instance, "channel_epoch": self.channel_epoch,
            "prior_channel_epoch": self.prior_channel_epoch, "profile_sha256": self.profile_sha256,
            "corral_manifest_sha256": self.corral_manifest_sha256, "joint_lock_sha256": self.joint_lock_sha256})
    }

    pub(super) fn link_ready(&self) -> Value {
        self.link_ready_for("corral")
    }

    /// The caller has strictly decoded the frame (including duplicate-key rejection).
    pub(super) fn check_link_ready(&self, value: Value) -> Result<()> {
        ensure!(
            value == self.link_ready_for("lite"),
            "fixture link_ready mismatch"
        );
        Ok(())
    }

    fn ready_record(&self) -> Value {
        json!({"v": 1, "type": "ready", "role": "corral", "link_id": self.link_id,
            "channel_epoch": self.channel_epoch, "profile_sha256": self.profile_sha256,
            "corral_manifest_sha256": self.corral_manifest_sha256, "joint_lock_sha256": self.joint_lock_sha256})
    }
}

pub(super) struct Resources {
    broker: OwnedFd,
    // Never a DTO, environment variable, or log field.
    credential: Vec<u8>,
    pending_ready: Option<PendingReady>,
    deadline: Instant,
}

struct PendingReady {
    pipe: OwnedFd,
    // Only a local CLOEXEC duplicate of the read end, not a driver write handle.
    lifetime: OwnedFd,
}

impl Resources {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn broker(&self) -> &OwnedFd {
        &self.broker
    }

    pub(super) fn credential(&self) -> &[u8] {
        &self.credential
    }

    pub(super) fn broker_handle(&self) -> Result<OwnedFd> {
        self.broker
            .try_clone()
            .context("retain fixture broker poll handle")
    }

    #[cfg(test)]
    pub(super) fn check_broker(&self) -> Result<()> {
        let mut fds = [PollFd::new(&self.broker, PollFlags::IN | PollFlags::RDHUP)];
        poll(&mut fds, Some(&Timespec::default())).context("poll fixture broker")?;
        ensure!(
            fds[0].revents().is_empty(),
            "fixture broker closed or unexpected data"
        );
        Ok(())
    }

    /// Called only after the durable enrollment and the successful hello response.
    pub(super) fn ready(
        &mut self,
        enrollment: &Enrollment,
        check_work: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let pending = self
            .pending_ready
            .take()
            .context("fixture ready already consumed")?;
        let mut record =
            serde_json::to_vec(&enrollment.ready_record()).context("encode fixture ready")?;
        record.push(b'\n');
        ensure!(record.len() <= PIPE_MAX, "fixture ready record too large");
        let scope = Scope {
            lifetime: &pending.lifetime,
            broker: &self.broker,
            deadline: self.deadline,
        };
        let events = scope.wait(&pending.pipe, PollFlags::OUT)?;
        ensure!(events == PollFlags::OUT, "fixture ready reader lost");
        check_work()?;
        // Linux PIPE_BUF is 4096. With a nonblocking pipe this short write is atomic.
        ensure!(
            io::write(&pending.pipe, &record).context("write fixture ready")? == record.len(),
            "short fixture ready write"
        );
        Ok(())
    }
}

impl Bootstrap {
    /// Take the prescribed descriptors exactly once. Failure is terminal for this CLI.
    ///
    /// # Safety
    /// Call from dedicated binary startup only. The calling process must exclusively own
    /// slots 3–8 (and slot 9 when a fault case is configured) with no wrappers or concurrent
    /// table mutation (including between inventory validation and ownership). The slots need not
    /// be open or contain valid inputs; the inventory checks that before taking ownership.
    pub(super) unsafe fn inherit(
        #[cfg(feature = "lite-analyst-faults")] case: Option<super::faults::Case>,
    ) -> Result<Self> {
        static CONSUMED: AtomicBool = AtomicBool::new(false);
        ensure!(
            !CONSUMED.swap(true, Ordering::Relaxed),
            "fixture launch already consumed"
        );
        let deadline = Instant::now() + BOOTSTRAP_TIMEOUT;
        #[cfg(feature = "lite-analyst-faults")]
        let fault_enabled = case.is_some();
        #[cfg(not(feature = "lite-analyst-faults"))]
        let fault_enabled = false;
        check_inventory(if fault_enabled { 10 } else { 9 })?;
        // SAFETY: F_GETFD verified these slots are open. The caller guarantees
        // the table stays stable and has no other Rust owners for the six slots.
        let [enrollment, work, broker, ready, lifetime, credential] =
            [3, 4, 5, 6, 7, 8].map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
        let fds = [&enrollment, &work, &broker, &ready, &lifetime, &credential];
        for fd in fds {
            io::fcntl_setfd(fd, FdFlags::CLOEXEC).context("set fixture descriptor CLOEXEC")?;
        }
        check_sandbox_basics()?;
        #[cfg(feature = "lite-analyst-faults")]
        let faults = case
            .map(|case| -> Result<_> {
                use super::faults::{FD, FaultControl};
                // SAFETY: inventory verified slot 9; the launch caller transfers it
                // exclusively with the same guarantee as slots 3–8.
                let fd = unsafe { OwnedFd::from_raw_fd(FD) };
                io::fcntl_setfd(&fd, FdFlags::CLOEXEC).context("set fixture barrier CLOEXEC")?;
                check_distinct(&[
                    &enrollment,
                    &work,
                    &broker,
                    &ready,
                    &lifetime,
                    &credential,
                    &fd,
                ])?;
                check_socket(&fd, SocketType::SEQPACKET)?;
                let flags = fs::fcntl_getfl(&fd).context("query fixture barrier flags")?;
                fs::fcntl_setfl(&fd, flags | OFlags::NONBLOCK)
                    .context("configure fixture barrier")?;
                Ok(FaultControl::new(case, fd))
            })
            .transpose()?;
        check_distinct(&fds)?;
        for fd in [&enrollment, &lifetime, &credential] {
            check_pipe(fd, OFlags::RDONLY)?;
        }
        check_pipe(&ready, OFlags::WRONLY)?;
        check_socket(&work, SocketType::STREAM)?;
        check_socket(&broker, SocketType::SEQPACKET)?;

        let scope = Scope {
            lifetime: &lifetime,
            broker: &broker,
            deadline,
        };
        let enrollment = parse_enrollment(&scope.line(enrollment, PIPE_MAX)?)?;
        let profile = read_metadata(PROFILE_PATH, PROFILE_MAX)?;
        let manifest = read_metadata(MANIFEST_PATH, MANIFEST_MAX)?;
        check_pins(&enrollment, &profile, &manifest)?;
        let credential = scope.line(credential, CREDENTIAL_LEN + 1)?;
        check_credential(&credential)?;
        let pending_ready = PendingReady {
            pipe: ready,
            lifetime: lifetime
                .try_clone()
                .context("retain fixture lifetime for ready")?,
        };
        Ok(Self {
            #[cfg(feature = "lite-analyst-faults")]
            faults,
            enrollment,
            work: UnixStream::from(work),
            lifetime,
            resources: Resources {
                broker,
                credential,
                pending_ready: Some(pending_ready),
                deadline,
            },
        })
    }
}

fn check_inventory(count: RawFd) -> Result<()> {
    // First check openness at the kernel FD table, independently of the procfs mount.
    // This also prevents the scan's directory open from filling a missing slot.
    for fd in 0..count {
        // SAFETY: The F_GETFD form accepts any integer and takes no further arguments.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(std::io::Error::last_os_error())
                .context("query inherited fixture descriptor");
        }
    }
    let fd = fs::open(
        "/proc/self/fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("open fixture descriptor inventory")?;
    ensure!(
        fs::fstatfs(&fd)
            .context("stat fixture descriptor inventory")?
            .f_type
            == fs::PROC_SUPER_MAGIC,
        "fixture descriptor inventory is not procfs"
    );
    let scan = fd.as_raw_fd();
    let mut seen = [false; 10];
    for entry in fs::Dir::new(fd).context("read fixture descriptor inventory")? {
        let entry = entry.context("scan fixture descriptor inventory")?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let number = name
            .to_str()
            .context("fixture descriptor name")?
            .parse::<RawFd>()
            .context("fixture descriptor number")?;
        if number == scan {
            continue;
        }
        ensure!(
            (0..count).contains(&number),
            "unexpected inherited fixture descriptor"
        );
        seen[number as usize] = true;
    }
    ensure!(
        seen[..count as usize].iter().all(|present| *present),
        "missing inherited fixture descriptor"
    );
    Ok(())
}

fn check_sandbox_basics() -> Result<()> {
    ensure!(
        !rustix::process::getuid().is_root() && !rustix::process::geteuid().is_root(),
        "fixture requires non-root uid"
    );
    ensure!(
        std::env::current_dir().context("fixture cwd unavailable")? == Path::new("/scratch"),
        "fixture requires /scratch cwd"
    );
    ensure!(
        rustix::thread::no_new_privs().context("query fixture no_new_privs")?,
        "fixture requires no_new_privs"
    );
    ensure!(
        rustix::thread::capabilities(None)
            .context("query fixture capabilities")?
            .effective
            .is_empty(),
        "fixture must not have effective capabilities"
    );
    Ok(())
}

fn check_distinct(fds: &[&OwnedFd]) -> Result<()> {
    let mut seen = HashSet::new();
    let mut insert = |fd: BorrowedFd<'_>| {
        let stat = fs::fstat(fd).context("stat fixture descriptor")?;
        Ok::<_, anyhow::Error>(seen.insert((stat.st_dev, stat.st_ino)))
    };
    // Standard streams may share one destination, but may not alias any role.
    for fd in [stdin().as_fd(), stdout().as_fd(), stderr().as_fd()] {
        insert(fd)?;
    }
    for fd in fds {
        ensure!(insert(fd.as_fd())?, "aliased fixture descriptors");
    }
    Ok(())
}

fn check_pipe(fd: &OwnedFd, access: OFlags) -> Result<()> {
    let stat = fs::fstat(fd).context("stat fixture pipe")?;
    let fs_type = fs::fstatfs(fd)
        .context("stat fixture pipe filesystem")?
        .f_type;
    ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::Fifo && fs_type == PIPEFS_MAGIC,
        "fixture descriptor requires anonymous pipe"
    );
    let flags = fs::fcntl_getfl(fd).context("query fixture pipe flags")?;
    ensure!(
        flags & OFlags::ACCMODE == access && !flags.contains(OFlags::PATH),
        "wrong fixture pipe direction"
    );
    fs::fcntl_setfl(fd, flags | OFlags::NONBLOCK).context("make fixture pipe nonblocking")?;
    Ok(())
}

fn check_socket(fd: &OwnedFd, kind: SocketType) -> Result<()> {
    ensure!(
        sockopt::socket_domain(fd).context("query fixture socket domain")? == AddressFamily::UNIX
            && sockopt::socket_type(fd).context("query fixture socket type")? == kind,
        "wrong fixture socket type"
    );
    let local = net::getsockname(fd).context("query fixture socket address")?;
    let peer = net::getpeername(fd)
        .context("query fixture socket peer")?
        .context("fixture socket lacks peer")?;
    for address in [local, peer] {
        // Linux returns just the family header for an unnamed Unix endpoint.
        // Preserve the actual kernel length; address conversions can erase it.
        ensure!(
            address.address_family() == AddressFamily::UNIX
                && address.addr_len() as usize == std::mem::offset_of!(libc::sockaddr_un, sun_path),
            "fixture socket must be unnamed"
        );
    }
    Ok(())
}

struct Scope<'a> {
    lifetime: &'a OwnedFd,
    broker: &'a OwnedFd,
    deadline: Instant,
}

impl Scope<'_> {
    fn remaining(&self) -> Result<Duration> {
        let time = self.deadline.saturating_duration_since(Instant::now());
        ensure!(!time.is_zero(), "fixture bootstrap deadline expired");
        Ok(time)
    }

    fn wait(&self, fd: &OwnedFd, events: PollFlags) -> Result<PollFlags> {
        let mut fds = [
            PollFd::new(self.lifetime, PollFlags::IN),
            PollFd::new(self.broker, PollFlags::IN | PollFlags::RDHUP),
            PollFd::new(fd, events),
        ];
        loop {
            let time = self.remaining()?;
            let timeout = Timespec {
                tv_sec: time.as_secs() as _,
                tv_nsec: time.subsec_nanos().into(),
            };
            match poll(&mut fds, Some(&timeout)) {
                Ok(0) | Err(Errno::INTR) => continue,
                result => result.context("poll fixture bootstrap descriptors")?,
            };
            self.remaining()?;
            // A lifetime pipe carries no data. Unexpected bytes also fence startup.
            ensure!(fds[0].revents().is_empty(), "fixture driver lifetime lost");
            ensure!(
                fds[1].revents().is_empty(),
                "fixture broker closed or unexpected data"
            );
            return Ok(fds[2].revents());
        }
    }

    fn line(&self, fd: OwnedFd, limit: usize) -> Result<Vec<u8>> {
        let mut bytes = vec![0; limit + 1];
        let mut len = 0;
        loop {
            let events = self.wait(&fd, PollFlags::IN)?;
            ensure!(
                !events.intersects(PollFlags::ERR | PollFlags::NVAL),
                "fixture bootstrap pipe error"
            );
            match io::read(&fd, &mut bytes[len..]) {
                Ok(0) => break,
                Ok(n) => {
                    len += n;
                    ensure!(len <= limit, "fixture bootstrap record too large");
                }
                Err(Errno::AGAIN | Errno::INTR) => continue,
                Err(error) => return Err(error).context("read fixture bootstrap pipe"),
            }
        }
        ensure!(
            len != 0
                && bytes[len - 1] == b'\n'
                && !bytes[..len - 1].iter().any(|b| matches!(b, b'\r' | b'\n')),
            "fixture bootstrap pipe requires one LF record"
        );
        bytes.truncate(len - 1);
        Ok(bytes)
    }
}

fn check_credential(credential: &[u8]) -> Result<()> {
    ensure!(
        credential.len() == CREDENTIAL_LEN && lower_hex(credential),
        "invalid fixture provider credential"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentRecord {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    role: String,
    link_id: String,
    lite_instance: String,
    corral_instance: String,
    channel_epoch: String,
    // Allow explicit null for a fresh link, but never treat an omitted field as null.
    #[serde(deserialize_with = "Deserialize::deserialize")]
    prior_channel_epoch: Option<String>,
    profile_sha256: String,
    corral_manifest_sha256: String,
    joint_lock_sha256: String,
}

fn parse_enrollment(bytes: &[u8]) -> Result<Enrollment> {
    let record: EnrollmentRecord = decode(bytes).context("invalid fixture enrollment")?;
    ensure!(
        record.v == 1 && record.kind == "enrollment" && record.role == "corral",
        "wrong fixture enrollment version or role"
    );
    let ids = [
        &record.link_id,
        &record.lite_instance,
        &record.corral_instance,
        &record.channel_epoch,
    ];
    let mut seen = HashSet::new();
    for id in ids.into_iter().chain(record.prior_channel_epoch.iter()) {
        ensure!(
            valid_id(id)
                && id.len() == 22
                && URL_SAFE_NO_PAD.decode(id).is_ok_and(|b| b.len() == 16)
                && seen.insert(id),
            "invalid fixture enrollment identity"
        );
    }
    ensure!(
        record.profile_sha256 == PROFILE_SHA256
            && sealed_digest(&record.corral_manifest_sha256)
            && sealed_digest(&record.joint_lock_sha256),
        "invalid fixture enrollment pin"
    );
    Ok(Enrollment {
        link_id: record.link_id,
        lite_instance: record.lite_instance,
        corral_instance: record.corral_instance,
        channel_epoch: record.channel_epoch,
        prior_channel_epoch: record.prior_channel_epoch,
        profile_sha256: record.profile_sha256,
        corral_manifest_sha256: record.corral_manifest_sha256,
        joint_lock_sha256: record.joint_lock_sha256,
    })
}

fn read_metadata(path: &str, cap: usize) -> Result<Vec<u8>> {
    let fd = fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .context("open pinned fixture metadata")?;
    let stat = fs::fstat(&fd).context("stat pinned fixture metadata")?;
    ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && (1..=cap as i64).contains(&stat.st_size),
        "invalid pinned fixture metadata type or size"
    );
    let mut bytes = Vec::new();
    File::from(fd)
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .context("read pinned fixture metadata")?;
    ensure!(
        bytes.len() == stat.st_size as usize,
        "pinned fixture metadata size changed"
    );
    Ok(bytes)
}

fn check_pins(enrollment: &Enrollment, profile: &[u8], manifest: &[u8]) -> Result<()> {
    ensure!(
        enrollment.profile_sha256 == PROFILE_SHA256
            && sha256(profile) == enrollment.profile_sha256
            && sha256(manifest) == enrollment.corral_manifest_sha256,
        "fixture metadata pin mismatch"
    );
    // Profile closure (including unknown-field rejection) follows from the independent
    // exact-byte pin. Cross-check the route and policy used by this daemon as well.
    let profile: Value = decode(profile).context("decode fixture launcher profile")?;
    let route = json!({"provider":"scripted","model":"fixture-v1","recipient":"synthetic-local","external":false,"tariff_revision":"demo-1"});
    ensure!(
        profile.pointer("/provider/route") == Some(&route)
            && profile.pointer("/pins/policy_ref").and_then(Value::as_str) == Some(POLICY_REF)
            && profile
                .pointer("/pins/prompt_sha256")
                .and_then(Value::as_str)
                == Some(PROMPT_SHA256)
            && profile
                .pointer("/pins/tool_policy_sha256")
                .and_then(Value::as_str)
                == Some(TOOL_POLICY_SHA256),
        "wrong fixture route or policy pin"
    );
    let manifest: Manifest = decode(manifest).context("decode fixture build manifest")?;
    manifest.check(&enrollment.profile_sha256)?;
    manifest.check_build_variant()
}

fn lower_hex(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

fn sealed_digest(value: &str) -> bool {
    // Repeated-digit placeholders appear in the template and bootstrap examples.
    valid_digest(value) && value.bytes().any(|b| b != value.as_bytes()[0])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    v: u64,
    kind: String,
    status: String,
    target: String,
    corral_revision: String,
    launcher_profile_sha256: String,
    build_recipe: String,
    host_requirements: HostRequirements,
    entrypoints: Entrypoints,
    components: BTreeMap<String, Component>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostRequirements {
    landlock_tcp_abi_min: u64,
    sysroot_abi: String,
    sandbox_backend: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entrypoints {
    daemon: String,
    pi: String,
    shim: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Component {
    role: Role,
    bytes: u64,
    sha256: String,
    executable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    Daemon,
    Pi,
    Shim,
    Runtime,
    Asset,
    BuildRecipe,
    Prompt,
    ToolPolicy,
}

impl Manifest {
    fn check_build_variant(&self) -> Result<()> {
        #[cfg(feature = "lite-analyst-faults")]
        {
            use super::faults::{CONTRACT, MANIFEST_PATH};
            ensure!(
                self.components
                    .get(MANIFEST_PATH)
                    .is_some_and(|item| item.role == Role::Asset
                        && !item.executable
                        && item.bytes == CONTRACT.len() as u64
                        && item.sha256 == sha256(CONTRACT)),
                "fault build contract mismatch"
            );
        }
        #[cfg(not(feature = "lite-analyst-faults"))]
        ensure!(
            !self
                .components
                .contains_key("provenance/corral-fault-v1.md"),
            "fault manifest requires separate build"
        );
        Ok(())
    }

    /// Check the sealed metadata, not the archive/tree (that remains runner-owned).
    fn check(&self, profile: &str) -> Result<()> {
        ensure!(
            self.v == 1
                && self.kind == "corral_lite_fixture_build"
                && self.status == "runnable"
                && self.target == "linux-x86_64"
                && self.launcher_profile_sha256 == profile,
            "not a runnable pinned fixture build"
        );
        ensure!(
            self.corral_revision.len() == 40
                && lower_hex(self.corral_revision.as_bytes())
                && self.corral_revision.bytes().any(|b| b != b'0'),
            "invalid fixture build revision"
        );
        let host = &self.host_requirements;
        ensure!(
            host.landlock_tcp_abi_min == 4
                && !host.sysroot_abi.trim().is_empty()
                && host.sysroot_abi.chars().count() <= 128
                && host.sysroot_abi != "example-not-a-runtime-pin"
                && matches!(
                    host.sandbox_backend.as_str(),
                    "rootless-bubblewrap" | "equivalent-rootless"
                ),
            "invalid fixture build host requirements"
        );
        ensure!(
            (7..=16_384).contains(&self.components.len()),
            "invalid fixture build component count"
        );
        for (path, component) in &self.components {
            ensure!(
                valid_path(path)
                    && (1..=1_073_741_824).contains(&component.bytes)
                    && sealed_digest(&component.sha256),
                "invalid fixture build component metadata"
            );
        }
        for (declared, expected, role) in [
            (&self.entrypoints.daemon, "bin/corrald-next", Role::Daemon),
            (&self.entrypoints.pi, "bin/corral-pi-lite-analyst", Role::Pi),
            (
                &self.entrypoints.shim,
                "bin/corral-lite-mcp-shim",
                Role::Shim,
            ),
        ] {
            ensure!(
                declared == expected
                    && self
                        .components
                        .get(declared)
                        .is_some_and(|c| c.role == role && c.executable),
                "invalid fixture build entrypoint"
            );
        }
        ensure!(
            self.components
                .get(&self.build_recipe)
                .is_some_and(|c| c.role == Role::BuildRecipe),
            "missing fixture build recipe"
        );
        for (role, digest) in [
            (Role::Prompt, PROMPT_SHA256),
            (Role::ToolPolicy, RAW_TOOL_POLICY_SHA256),
        ] {
            let mut assets = self.components.values().filter(|c| c.role == role);
            ensure!(
                assets.next().is_some_and(|c| c.sha256 == digest) && assets.next().is_none(),
                "wrong fixture build policy asset"
            );
        }
        Ok(())
    }
}

fn valid_path(path: &str) -> bool {
    path.len() <= 256
        && path.split('/').all(|part| {
            let mut bytes = part.bytes();
            bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
