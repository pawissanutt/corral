use corral_protocol::device::{NOISE_PAIR, NOISE_SESSION};
use snow::{Builder, HandshakeState, Keypair};
#[cfg(target_arch = "wasm32")]
use snow::{
    params::{CipherChoice, DHChoice, HashChoice},
    resolvers::{CryptoResolver, DefaultResolver},
    types::{Cipher, Dh, Hash, Random},
};

#[cfg(target_arch = "wasm32")]
struct BrowserRandom;

#[cfg(target_arch = "wasm32")]
impl Random for BrowserRandom {
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), snow::Error> {
        let window = web_sys::window().ok_or(snow::Error::Rng)?;
        let crypto = window.crypto().map_err(|_| snow::Error::Rng)?;
        crypto
            .get_random_values_with_u8_array(dest)
            .map(|_| ())
            .map_err(|_| snow::Error::Rng)
    }
}

#[cfg(target_arch = "wasm32")]
struct BrowserResolver;

#[cfg(target_arch = "wasm32")]
impl CryptoResolver for BrowserResolver {
    fn resolve_rng(&self) -> Option<Box<dyn Random>> {
        Some(Box::new(BrowserRandom))
    }

    fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
        DefaultResolver.resolve_dh(choice)
    }

    fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
        DefaultResolver.resolve_hash(choice)
    }

    fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
        DefaultResolver.resolve_cipher(choice)
    }
}

pub(super) fn generate_keypair() -> Result<Keypair, String> {
    builder(NOISE_SESSION)?
        .generate_keypair()
        .map_err(crypto_error)
}

pub(super) fn initiator_session(
    private: &[u8; 32],
    daemon: &[u8; 32],
) -> Result<HandshakeState, String> {
    builder(NOISE_SESSION)?
        .local_private_key(private)
        .and_then(|builder| builder.remote_public_key(daemon))
        .and_then(|builder| builder.build_initiator())
        .map_err(crypto_error)
}

pub(super) fn initiator_pair(
    private: &[u8; 32],
    daemon: &[u8; 32],
    token: &[u8; 32],
) -> Result<HandshakeState, String> {
    builder(NOISE_PAIR)?
        .local_private_key(private)
        .and_then(|builder| builder.remote_public_key(daemon))
        .and_then(|builder| builder.psk(2, token))
        .and_then(|builder| builder.build_initiator())
        .map_err(crypto_error)
}

fn builder<'a>(pattern: &str) -> Result<Builder<'a>, String> {
    let params = pattern.parse().map_err(crypto_error)?;
    #[cfg(target_arch = "wasm32")]
    let builder = Builder::with_resolver(params, Box::new(BrowserResolver));
    #[cfg(windows)]
    let builder = Builder::new(params);
    Ok(builder)
}

#[cfg(target_arch = "wasm32")]
fn crypto_error(error: snow::Error) -> String {
    if matches!(error, snow::Error::Rng) {
        "Browser cryptographic randomness is unavailable; open Corral on HTTPS or localhost in a browser with WebCrypto support.".into()
    } else {
        format!("Could not initialise browser encryption: {error}")
    }
}

#[cfg(windows)]
fn crypto_error(error: snow::Error) -> String {
    format!("Could not initialise device encryption: {error}")
}
