//! Current-user DPAPI and a durable, authoritative Forget state for Windows pairing.
use super::socket::normalize_endpoint;
use crate::client::durable_native::windows::{LockedFile, state_root};
use serde::{Deserialize, Serialize};
use std::{io, path::Path, ptr, slice};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct WebCredential {
    pub(super) v: u32,
    pub(super) device: String,
    pub(super) name: String,
    pub(super) private: String,
    pub(super) daemon: String,
    pub(super) port: u16,
    #[serde(default)]
    pub(super) endpoint: Option<String>,
}

pub(super) type Stored = super::identity::Stored<WebCredential>;

const NAME: &str = "pairing.dpapi";
const LOCK: &str = "pairing.lock";
const MISSING: &[u8] = b"corral-pairing-v1:missing";
const FORGET_PENDING: &[u8] = b"corral-pairing-v1:forget-pending";
const MAX_BYTES: usize = 16 * 1024;
const TOO_LARGE: &str = "the encoded pairing exceeds 16 KiB";

fn storage_error(action: &str, error: impl std::fmt::Display) -> String {
    format!("Could not {action} this Windows device's pairing: {error}")
}

fn invalid_credential() -> String {
    "This Windows device's stored pairing is damaged or unsupported; Forget this device and pair it again.".into()
}

fn open(root: &Path) -> Result<LockedFile, String> {
    LockedFile::lock(root, NAME, LOCK).map_err(|error| storage_error("lock", error))
}

async fn with_file<T: Send + 'static>(
    operation: impl FnOnce(&LockedFile) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    nsh_core::prelude::blocking(move || operation(&open(&state_root()?)?))
        .await
        .map_err(|error| storage_error("access", error))?
}

pub(super) async fn load() -> Result<Stored, String> {
    with_file(load_locked).await
}

pub(super) async fn save(credential: &WebCredential) -> Result<(), String> {
    let mut plaintext = encode(credential)?;
    with_file(move |file| save_locked(file, &mut plaintext)).await
}

pub(super) async fn begin_forget() -> Result<(), String> {
    with_file(begin_forget_locked).await
}

pub(super) async fn clear() -> Result<(), String> {
    with_file(clear_locked).await
}

pub(super) async fn finish_forget() -> Result<(), String> {
    with_file(finish_forget_locked).await
}

fn validate(credential: &WebCredential) -> Result<(), String> {
    fn valid_key(key: &str) -> bool {
        key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
    if credential.v != 1
        || credential.device.is_empty()
        || !valid_key(&credential.private)
        || !valid_key(&credential.daemon)
    {
        return Err(invalid_credential());
    }
    let endpoint = credential
        .endpoint
        .as_deref()
        .ok_or_else(invalid_credential)?;
    if normalize_endpoint(endpoint).is_ok_and(|normalized| normalized == endpoint) {
        Ok(())
    } else {
        Err(invalid_credential())
    }
}

fn encode(credential: &WebCredential) -> Result<Zeroizing<Vec<u8>>, String> {
    validate(credential)?;
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, credential).map_err(|_| invalid_credential())?;
    Ok(bytes)
}

enum Encoded {
    Missing,
    ForgetPending,
    Protected(Vec<u8>),
}

fn read(file: &LockedFile) -> Result<Encoded, String> {
    match file
        .read(MAX_BYTES, TOO_LARGE)
        .map_err(|error| storage_error("read", error))?
    {
        None => Ok(Encoded::Missing),
        Some(bytes) if bytes == MISSING => Ok(Encoded::Missing),
        Some(bytes) if bytes == FORGET_PENDING => Ok(Encoded::ForgetPending),
        Some(bytes) => Ok(Encoded::Protected(bytes)),
    }
}

fn load_locked(file: &LockedFile) -> Result<Stored, String> {
    match read(file)? {
        Encoded::Missing => Ok(Stored::Missing),
        Encoded::ForgetPending => Ok(Stored::ForgetPending),
        Encoded::Protected(mut ciphertext) => Ok(Stored::Credential(decode(&mut ciphertext)?)),
    }
}

fn decode(ciphertext: &mut [u8]) -> Result<WebCredential, String> {
    let plaintext = dpapi(ciphertext, Mode::Decrypt)?;
    let credential = serde_json::from_slice(plaintext.bytes()).map_err(|_| invalid_credential())?;
    validate(&credential)?;
    Ok(credential)
}

fn save_locked(file: &LockedFile, plaintext: &mut [u8]) -> Result<(), String> {
    match read(file)? {
        Encoded::Missing => (),
        Encoded::ForgetPending => {
            return Err(
                "Forgetting this Windows device is incomplete; retry Forget before pairing.".into(),
            );
        }
        // A damaged record might be a damaged tombstone. Only explicit Forget can replace it.
        Encoded::Protected(mut ciphertext) => {
            decode(&mut ciphertext)?;
        }
    }
    let encrypted = dpapi(plaintext, Mode::Encrypt)?;
    file.replace(encrypted.bytes())
        .map_err(|error| storage_error("save", error))
}

fn begin_forget_locked(file: &LockedFile) -> Result<(), String> {
    // One atomic replacement both commits the intent and retires the DPAPI blob. This
    // must also work when the current blob is too large, damaged, or no longer decryptable.
    file.replace(FORGET_PENDING)
        .map_err(|error| storage_error("record Forget for", error))
}

fn clear_locked(file: &LockedFile) -> Result<(), String> {
    // Unlike the browser's two IndexedDB records, begin already retired the blob.
    // Check that another instance did not finish Forget and then pair anew.
    match read(file)? {
        Encoded::Missing | Encoded::ForgetPending => Ok(()),
        Encoded::Protected(_) => Err(pairing_changed()),
    }
}

fn finish_forget_locked(file: &LockedFile) -> Result<(), String> {
    match read(file)? {
        Encoded::Missing => Ok(()),
        Encoded::ForgetPending => file
            .replace(MISSING)
            .map_err(|error| storage_error("finish Forget for", error)),
        Encoded::Protected(_) => Err(pairing_changed()),
    }
}

fn pairing_changed() -> String {
    "This Windows device's pairing changed during Forget; retry Forget.".into()
}

enum Mode {
    Encrypt,
    Decrypt,
}

struct Blob {
    data: CRYPT_INTEGER_BLOB,
    plaintext: bool,
}

impl Blob {
    fn bytes(&self) -> &[u8] {
        // SAFETY: only called on successfully validated output from DPAPI, which allocated
        // data for the whole range. The blob keeps the allocation alive for the borrow.
        unsafe { slice::from_raw_parts(self.data.pbData, self.data.cbData as usize) }
    }
}

impl Drop for Blob {
    fn drop(&mut self) {
        if !self.data.pbData.is_null() {
            // SAFETY: DPAPI allocates this buffer using LocalAlloc. The blob owns it and
            // `plaintext` is set only when a successful decryption provided its length.
            unsafe {
                if self.plaintext {
                    slice::from_raw_parts_mut(self.data.pbData, self.data.cbData as usize)
                        .zeroize();
                }
                LocalFree(self.data.pbData.cast());
            }
        }
    }
}

fn dpapi(bytes: &mut [u8], mode: Mode) -> Result<Blob, String> {
    if bytes.is_empty() {
        return Err(invalid_credential());
    }
    if bytes.len() > MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut result = Blob {
        data: CRYPT_INTEGER_BLOB::default(),
        plaintext: false,
    };
    // SAFETY: all optional pointers are null; the live input borrows `bytes`, and the
    // output struct stays writable for the call. No LOCAL_MACHINE flag: only this user
    // may unwrap the protected blob. Do not let DPAPI open a credential prompt.
    let status = unsafe {
        match mode {
            Mode::Encrypt => CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut result.data,
            ),
            Mode::Decrypt => CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut result.data,
            ),
        }
    };
    if status == 0 {
        let action = match mode {
            Mode::Encrypt => "protect",
            Mode::Decrypt => "unlock",
        };
        return Err(storage_error(action, io::Error::last_os_error()));
    }
    result.plaintext =
        matches!(mode, Mode::Decrypt) && result.data.cbData as usize <= isize::MAX as usize;
    if result.data.cbData as usize > MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    if result.data.pbData.is_null() || result.data.cbData == 0 {
        return Err(invalid_credential());
    }
    Ok(result)
}

#[cfg(test)]
#[path = "keystore_windows_tests.rs"]
mod tests;
