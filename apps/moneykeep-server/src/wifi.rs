//! Wi-Fi capture (mobile-capture.md revision 10): phones on the home or office
//! network reach the capture server directly, over HTTPS, with no Cloudflare account.
//!
//! - **The certificate authority (CA)** made on this computer: ECDSA P-256, valid 10
//!   years, "MoneyKeep Capture CA (<computer name>)", basicConstraints CA (no
//!   intermediates), key usage certificate signing only, and **name constraints**
//!   (RFC 5280 section 4.2.1.10) permitting only the chosen IP address (and the local
//!   name when there is one; otherwise the reserved name `invalid`, RFC 2606, so no
//!   DNS name passes) and excluding every IPv6 address, so trusting it on a phone
//!   cannot vouch for any other site.
//! - **The server certificate**: P-256, valid 825 days from one day before issue
//!   (small clock differences), subjectAltName the IP address and the local name;
//!   renewed by the server 30 days before it ends, with the same CA.
//! - **The keys** in the credential store or sealed files ([`crate::secrets`]).
//! - **The HTTPS listener** on `<address>:14443`: TLS (rustls with ring) in front of
//!   the same request handling as the loopback listener, which runs on a private
//!   loopback port; the Access check does not apply there (the device token does).
//! - **The helper** on `<address>:14080`, plain HTTP: the certificate page `/`, the CA
//!   certificate `/ca.crt` (Android) and `/moneykeep-ca.mobileconfig` (iPhone/iPad, a
//!   configuration profile with a `com.apple.security.root` payload, Apple
//!   Configuration Profile Reference); everything else redirects to HTTPS.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine;
use mk_capture_api as api;
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    NameConstraints, SanType, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::secrets::Secrets;
use crate::store::{self, StoreResult};

pub const CA_CERT_FILE: &str = "wifi-ca.crt";
pub const SERVER_CERT_FILE: &str = "wifi-server.crt";
pub const INFO_FILE: &str = "wifi.json";
/// The names of the two private keys ([`Secrets`]).
pub const CA_KEY: &str = "wifi-ca-key";
pub const TLS_KEY: &str = "wifi-tls-key";

const DAY: i64 = 86_400;
pub const CA_DAYS: i64 = 3652;
pub const SERVER_DAYS: i64 = 825;
pub const RENEW_DAYS: i64 = 30;
/// Phones connected at once over Wi-Fi (each connection has a relay thread).
const MAX_CONNECTIONS: usize = 64;
const IDLE: Duration = Duration::from_secs(120);

/// The address phones use: the IPv4 address and the optional local name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub ip: Ipv4Addr,
    #[serde(default)]
    pub name: Option<String>,
}

impl Target {
    /// The host phones open: the local name when there is one, else the IP address.
    pub fn host(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.ip.to_string())
    }
}

/// A local name as typed: lower case, letters, digits, dashes and dots, with at
/// least one dot (e.g. `moneykeep.home.arpa`, RFC 8375).
pub fn check_name(name: &str) -> Result<Option<String>, String> {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() {
        return Ok(None);
    }
    let ok = n.len() <= 253
        && n.contains('.')
        && n.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && n.parse::<IpAddr>().is_err();
    if ok {
        Ok(Some(n))
    } else {
        Err(format!(
            "\"{}\" is not a local name such as moneykeep.home.arpa.",
            name.trim()
        ))
    }
}

/// An IPv4 address of a private network (RFC 1918) or link-local, as Wi-Fi capture
/// needs.
pub fn check_address(text: &str) -> Result<Ipv4Addr, String> {
    let ip: Ipv4Addr = text.trim().parse().map_err(|_| {
        format!(
            "\"{}\" is not an IPv4 address such as 192.168.1.20.",
            text.trim()
        )
    })?;
    // Loopback addresses serve only this computer (tests, or trying it out).
    if ip.is_private() || ip.is_link_local() || ip.is_loopback() {
        Ok(ip)
    } else {
        Err(format!(
            "{ip} is not a home or office network address (10.x, 172.16–31.x or 192.168.x)."
        ))
    }
}

/// This computer has `ip` (binding a socket to it works only then).
pub fn address_present(ip: Ipv4Addr) -> bool {
    std::net::UdpSocket::bind((ip, 0)).is_ok()
}

/// A port is free on `ip`: `Ok`, or the message naming it with the next free one.
pub fn check_port(ip: Ipv4Addr, port: u16, other: u16) -> Result<(), String> {
    if TcpListener::bind((ip, port)).is_ok() {
        return Ok(());
    }
    let next = (port.saturating_add(1)..=65535)
        .find(|p| *p != other && TcpListener::bind((ip, *p)).is_ok());
    Err(match next {
        Some(n) => format!(
            "Port {port} is already used by another program on this computer. Use port {n} instead."
        ),
        None => format!("Port {port} is already used by another program on this computer."),
    })
}

/// The computer's name for the CA's name.
pub fn computer_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    clean_computer(&raw)
}

fn clean_computer(raw: &str) -> String {
    let c: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        .take(40)
        .collect();
    if c.trim().is_empty() {
        "this computer".to_owned()
    } else {
        c.trim().to_owned()
    }
}

/// What was made, kept next to the certificates (public facts only).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Info {
    pub ca_name: String,
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    pub ca_not_before: i64,
    pub ca_not_after: i64,
    pub cert_not_before: i64,
    pub cert_not_after: i64,
    pub fingerprint: String,
}

/// The CA's SHA-256 fingerprint as 8 groups of 4 upper-case hex digits (its first
/// 128 bits), for comparing by eye with what the phone shows.
pub fn fingerprint(der: &[u8]) -> String {
    let h = Sha256::digest(der);
    h.get(..16)
        .unwrap_or(&[])
        .chunks(2)
        .map(|c| store::hex(c).to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn when(secs: i64) -> Result<time::OffsetDateTime, String> {
    time::OffsetDateTime::from_unix_timestamp(secs).map_err(|e| e.to_string())
}

fn serial() -> StoreResult<SerialNumber> {
    let mut b = store::random::<16>()?;
    if let Some(first) = b.first_mut() {
        *first &= 0x7f;
    }
    Ok(SerialNumber::from_slice(&b))
}

fn rc(e: rcgen::Error) -> String {
    format!("the certificate could not be made: {e}")
}

/// The CA's settings (also rebuilt to sign a renewal: the issuer's name, key
/// identifier method and key usages come from here).
pub fn ca_params(
    ca_name: &str,
    target: &Target,
    not_before: i64,
) -> Result<CertificateParams, String> {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, ca_name);
    dn.push(DnType::OrganizationName, "MoneyKeep");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    p.not_before = when(not_before)?;
    p.not_after = when(not_before.saturating_add(CA_DAYS * DAY))?;
    let dns = target.name.clone().unwrap_or_else(|| "invalid".to_owned());
    p.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix(target.ip.octets(), 32)),
            GeneralSubtree::DnsName(dns),
        ],
        excluded_subtrees: vec![GeneralSubtree::IpAddress(CidrSubnet::from_v6_prefix(
            [0; 16], 0,
        ))],
    });
    Ok(p)
}

/// A new CA: its certificate (DER) and private key (PKCS#8).
pub fn new_ca(
    ca_name: &str,
    target: &Target,
    now: i64,
) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>), String> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(rc)?;
    let mut p = ca_params(ca_name, target, now - DAY)?;
    p.serial_number = Some(serial()?);
    let cert = p.self_signed(&key).map_err(rc)?;
    Ok((cert.der().to_vec(), Zeroizing::new(key.serialize_der())))
}

/// A new server certificate signed by the CA: its certificate (DER) and private key.
pub fn new_server_cert(
    ca_name: &str,
    ca_target: &Target,
    ca_not_before: i64,
    ca_key: &[u8],
    target: &Target,
    now: i64,
) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>), String> {
    let ca_pair = KeyPair::from_pkcs8_der_and_sign_algo(
        &PrivatePkcs8KeyDer::from(ca_key),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map_err(rc)?;
    let issuer = Issuer::new(ca_params(ca_name, ca_target, ca_not_before)?, ca_pair);
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(rc)?;
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, target.host());
    p.distinguished_name = dn;
    p.subject_alt_names = vec![SanType::IpAddress(IpAddr::V4(target.ip))];
    if let Some(n) = &target.name {
        p.subject_alt_names
            .push(SanType::DnsName(n.clone().try_into().map_err(rc)?));
    }
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    p.serial_number = Some(serial()?);
    let start = now - DAY;
    p.not_before = when(start)?;
    p.not_after = when(start.saturating_add(SERVER_DAYS * DAY))?;
    let cert = p.signed_by(&key, &issuer).map_err(rc)?;
    Ok((cert.der().to_vec(), Zeroizing::new(key.serialize_der())))
}

/// PEM text of a certificate.
pub fn pem(der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

/// The DER of PEM text (one certificate).
pub fn from_pem(text: &str) -> Option<Vec<u8>> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .concat();
    base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()
}

/// The certificates and the server key in use.
pub struct Material {
    pub info: Info,
    pub ca_der: Vec<u8>,
    pub cert_der: Vec<u8>,
    pub key: Zeroizing<Vec<u8>>,
    /// A new CA was made by this call.
    pub ca_created: bool,
    /// A new server certificate was made by this call.
    pub renewed: bool,
}

impl std::fmt::Debug for Material {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Material")
            .field("info", &self.info)
            .field("key", &"(hidden)")
            .finish_non_exhaustive()
    }
}

pub fn read_info(dir: &Path) -> Option<Info> {
    std::fs::read(dir.join(INFO_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

fn read_cert(dir: &Path, file: &str) -> Option<Vec<u8>> {
    std::fs::read_to_string(dir.join(file))
        .ok()
        .and_then(|t| from_pem(&t))
}

/// Make what is missing for `target` and renew what is due: a new CA when there is
/// none, it ends, or the address (its name constraints) changed; a new server
/// certificate with a new CA, a changed address, or within 30 days of its end.
///
/// Wi-Fi setup only (it may make a new certificate authority, which every phone must
/// install again); the server itself uses [`ensure_existing`].
pub fn ensure(
    dir: &Path,
    secrets: &Secrets,
    target: &Target,
    computer: &str,
    now: i64,
) -> Result<Material, String> {
    ensure_with(dir, secrets, target, computer, now, true)
}

/// upgrade-resilience-1 U1.2: Wi-Fi capture's damaged certificate files, as health's
/// `wifi_error` and the desktop's Status card say it.
pub const DAMAGED: &str = "Wi-Fi capture can't start: its certificate files are damaged";

/// What the server uses at start and when renewing (U1.2): the certificate
/// authority is never replaced here. When `wifi.json`, the CA certificate or the CA
/// key can't be read (or they are for another address, or the CA ended), [`DAMAGED`]
/// (with why) and nothing is written; the server certificate is renewed when due.
pub fn ensure_existing(
    dir: &Path,
    secrets: &Secrets,
    target: &Target,
    now: i64,
) -> Result<Material, String> {
    ensure_with(dir, secrets, target, "", now, false)
}

fn ensure_with(
    dir: &Path,
    secrets: &Secrets,
    target: &Target,
    computer: &str,
    now: i64,
    may_make_ca: bool,
) -> Result<Material, String> {
    let old = read_info(dir);
    let ca_key = match secrets.get(CA_KEY) {
        Ok(k) => k,
        // Setup replaces a key that can't be opened; the server never does.
        Err(_) if may_make_ca => None,
        Err(e) => return Err(format!("{DAMAGED} ({e})")),
    };
    let ca_der = read_cert(dir, CA_CERT_FILE);
    let same_place = |i: &Info| i.address == target.ip.to_string() && i.name == target.name;
    let (mut info, ca_der, ca_key, ca_created) = match (old, ca_key, ca_der) {
        (Some(i), Some(k), Some(d)) if same_place(&i) && now < i.ca_not_after - DAY => {
            (i, d, k, false)
        }
        (old, key, der) if !may_make_ca => {
            let why = match (old, key, der) {
                (None, ..) => format!("{INFO_FILE} can't be read"),
                (_, None, _) => "the certificate authority's key is missing".to_owned(),
                (_, _, None) => format!("{CA_CERT_FILE} can't be read"),
                (Some(i), ..) if !same_place(&i) => {
                    format!("the certificate is for {}, not {}", i.address, target.ip)
                }
                _ => "the certificate authority has ended".to_owned(),
            };
            return Err(format!("{DAMAGED} ({why})"));
        }
        _ => {
            let ca_name = format!("MoneyKeep Capture CA ({})", clean_computer(computer));
            let (der, key) = new_ca(&ca_name, target, now)?;
            secrets.set(CA_KEY, &key)?;
            store::write_private(&dir.join(CA_CERT_FILE), pem(&der).as_bytes())?;
            let info = Info {
                ca_name,
                address: target.ip.to_string(),
                name: target.name.clone(),
                ca_not_before: now - DAY,
                ca_not_after: now - DAY + CA_DAYS * DAY,
                cert_not_before: 0,
                cert_not_after: 0,
                fingerprint: fingerprint(&der),
            };
            (info, der, key, true)
        }
    };
    let cert = read_cert(dir, SERVER_CERT_FILE);
    // A damaged server key is simply made again (phones trust the CA, not it).
    let tls = secrets.get(TLS_KEY).ok().flatten();
    let due = now >= info.cert_not_after - RENEW_DAYS * DAY;
    let (cert_der, key, renewed) = match (cert, tls) {
        (Some(c), Some(k)) if !ca_created && !due => (c, k, false),
        _ => {
            let ca_target = Target {
                ip: info
                    .address
                    .parse()
                    .map_err(|_| "wifi.json is damaged".to_owned())?,
                name: info.name.clone(),
            };
            let (der, key) = new_server_cert(
                &info.ca_name,
                &ca_target,
                info.ca_not_before,
                &ca_key,
                target,
                now,
            )?;
            secrets.set(TLS_KEY, &key)?;
            store::write_private(&dir.join(SERVER_CERT_FILE), pem(&der).as_bytes())?;
            info.cert_not_before = now - DAY;
            info.cert_not_after = now - DAY + SERVER_DAYS * DAY;
            (der, key, true)
        }
    };
    if ca_created || renewed {
        let json = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
        store::write_private(&dir.join(INFO_FILE), &json)?;
    }
    Ok(Material {
        info,
        ca_der,
        cert_der,
        key,
        ca_created,
        renewed,
    })
}

/// Remove the certificates and keys (Remove Wi-Fi capture).
pub fn remove(dir: &Path, secrets: &Secrets) -> Vec<String> {
    let mut done = Vec::new();
    for f in [CA_CERT_FILE, SERVER_CERT_FILE, INFO_FILE] {
        if std::fs::remove_file(dir.join(f)).is_ok() {
            done.push(format!("{f} removed"));
        }
    }
    for k in [CA_KEY, TLS_KEY] {
        if secrets.delete(k).is_ok() {
            done.push(format!("{k} removed"));
        }
    }
    done
}

// ----- the iPhone profile and the certificate page ------------------------------------------

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A UUID (version 4 form) from 16 bytes.
fn uuid(b: [u8; 16]) -> String {
    let h = store::hex(&b).to_ascii_uppercase();
    let p = |a: usize, z: usize| h.get(a..z).unwrap_or("").to_owned();
    let v = format!("4{}", h.get(13..16).unwrap_or(""));
    format!("{}-{}-{}-{}-{}", p(0, 8), p(8, 12), v, p(16, 20), p(20, 32))
}

/// The iPhone/iPad configuration profile carrying the CA certificate (payload type
/// `com.apple.security.root`). The identifiers come from the certificate, so the
/// same CA always gives the same profile.
pub fn mobileconfig(ca_der: &[u8], ca_name: &str) -> String {
    let h = Sha256::digest(ca_der);
    let mut a = [0u8; 16];
    let mut b = [0u8; 16];
    a.copy_from_slice(h.get(..16).unwrap_or(&[0; 16]));
    b.copy_from_slice(h.get(16..32).unwrap_or(&[0; 16]));
    let data = base64::engine::general_purpose::STANDARD.encode(ca_der);
    let id = store::hex(h.get(..6).unwrap_or(&[]));
    let name = xml(ca_name);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadCertificateFileName</key>
      <string>moneykeep-ca.cer</string>
      <key>PayloadContent</key>
      <data>{data}</data>
      <key>PayloadDescription</key>
      <string>Lets this iPhone or iPad trust MoneyKeep phone capture on your computer's Wi-Fi address only.</string>
      <key>PayloadDisplayName</key>
      <string>{name}</string>
      <key>PayloadIdentifier</key>
      <string>com.moneykeep.capture.ca.{id}.cert</string>
      <key>PayloadType</key>
      <string>com.apple.security.root</string>
      <key>PayloadUUID</key>
      <string>{u1}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>PayloadDescription</key>
  <string>Installs the certificate of MoneyKeep phone capture on your computer. It is valid only for your computer's Wi-Fi address.</string>
  <key>PayloadDisplayName</key>
  <string>{name}</string>
  <key>PayloadIdentifier</key>
  <string>com.moneykeep.capture.ca.{id}</string>
  <key>PayloadRemovalDisallowed</key>
  <false/>
  <key>PayloadType</key>
  <string>Configuration</string>
  <key>PayloadUUID</key>
  <string>{u2}</string>
  <key>PayloadVersion</key>
  <integer>1</integer>
</dict>
</plist>
"#,
        u1 = uuid(a),
        u2 = uuid(b),
    )
}

/// Which phone asked (by its browser's User-Agent): iPhone/iPad or not.
pub fn is_apple(user_agent: &str) -> bool {
    ["iPhone", "iPad", "iPod"]
        .iter()
        .any(|k| user_agent.contains(k))
}

/// The Android steps on the certificate page.
pub const ANDROID_STEPS: [&str; 6] = [
    "Tap <b>Download certificate (Android)</b> above. If Chrome asks, keep the file.",
    "Open <b>Settings</b> &gt; <b>Security</b> (or <b>Security &amp; privacy</b>) &gt; <b>More security settings</b> &gt; <b>Encryption &amp; credentials</b>. The words differ a little between phone makers; searching Settings for “certificate” also finds it.",
    "Tap <b>Install a certificate</b> &gt; <b>CA certificate</b>, then <b>Install anyway</b>.",
    "Choose the downloaded file <b>moneykeep-ca.crt</b> (in Downloads). The phone may ask for its screen lock.",
    "Check under <b>Trusted credentials</b> &gt; <b>User</b> that “MoneyKeep Capture CA” is listed.",
    "Come back here and scan the second QR code in MoneyKeep to pair this phone.",
];

/// The iPhone/iPad steps on the certificate page.
pub const APPLE_STEPS: [&str; 6] = [
    "Open this page in <b>Safari</b> (the Camera app opens Safari).",
    "Tap <b>Download certificate (iPhone or iPad)</b> above, then <b>Allow</b>.",
    "Open <b>Settings</b>: tap <b>Profile Downloaded</b> near the top (or <b>General</b> &gt; <b>VPN &amp; Device Management</b>), then <b>Install</b>, and enter the phone's passcode.",
    "Open <b>Settings</b> &gt; <b>General</b> &gt; <b>About</b> &gt; <b>Certificate Trust Settings</b> and turn on full trust for “MoneyKeep Capture CA”.",
    "Compare the fingerprint below with the one MoneyKeep shows on your computer.",
    "Come back to MoneyKeep and scan the second QR code to pair this phone.",
];

fn steps(title: &str, list: &[&str]) -> String {
    let items: String = list.iter().map(|s| format!("<li>{s}</li>")).collect();
    format!("<section><h2>{title}</h2><ol>{items}</ol></section>")
}

/// The certificate-install page (helper port `/`): the fingerprint, both download
/// buttons and the steps for the phone that asked first, then the other.
pub fn install_page(info: &Info, apple_first: bool) -> String {
    let android = steps("Android phone or tablet", &ANDROID_STEPS);
    let apple = steps("iPhone or iPad", &APPLE_STEPS);
    let (first, second) = if apple_first {
        (apple, android)
    } else {
        (android, apple)
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>MoneyKeep capture — certificate</title>
<style>
:root {{ --bg:#f6f7f9; --card:#ffffff; --text:#1b1f24; --muted:#56606b; --accent:#1f5fbf; --line:#d8dde3; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#15181c; --card:#20252b; --text:#eef1f4; --muted:#aab3bd; --accent:#7fb0ff; --line:#3a424b; }} }}
body {{ margin:0; background:var(--bg); color:var(--text); font:17px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }}
main {{ max-width:480px; margin:0 auto; padding:16px; }}
h1 {{ font-size:22px; margin:8px 0 4px; }} h2 {{ font-size:18px; margin:20px 0 6px; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:14px 16px; margin:12px 0; }}
.muted {{ color:var(--muted); font-size:15px; }}
.fp {{ font:600 17px/1.6 ui-monospace, "SF Mono", Menlo, Consolas, monospace; letter-spacing:.5px; word-spacing:4px; }}
a.button {{ display:block; text-align:center; text-decoration:none; background:var(--accent); color:#fff; font-weight:600; border-radius:10px; padding:14px; margin:10px 0; min-height:24px; }}
@media (prefers-color-scheme: dark) {{ a.button {{ color:#0d1117; }} }}
ol {{ padding-left:22px; }} li {{ margin:6px 0; }}
</style></head>
<body><main>
<h1>MoneyKeep capture</h1>
<p class="muted">Step 1 of 2 on this phone: trust your computer's certificate, so the capture app can open securely at <b>https://{host}:{port}</b>.</p>
<div class="card">
<div class="muted">Certificate</div><div><b>{ca}</b></div>
<div class="muted" style="margin-top:8px">Fingerprint (SHA-256, first 32 digits) — it must match the one MoneyKeep shows:</div>
<div class="fp">{fp}</div>
<a class="button" href="{crt}">Download certificate (Android)</a>
<a class="button" href="{mc}">Download certificate (iPhone or iPad)</a>
<p class="muted">The certificate is only valid for your computer's address ({ip}{name}). It cannot be used for any other website.</p>
</div>
<div class="card">{first}</div>
<div class="card">{second}</div>
<p class="muted">Already installed? Open <a href="https://{host}:{port}/">https://{host}:{port}/</a>.</p>
</main></body></html>
"#,
        host = xml(&info.name.clone().unwrap_or_else(|| info.address.clone())),
        port = "{HTTPS_PORT}",
        ca = xml(&info.ca_name),
        fp = xml(&info.fingerprint),
        crt = api::CA_CRT_PATH,
        mc = api::CA_MOBILECONFIG_PATH,
        ip = xml(&info.address),
        name = info
            .name
            .as_ref()
            .map(|n| format!(" and {}", xml(n)))
            .unwrap_or_default(),
    )
}

/// The certificate page for a phone, with the HTTPS port filled in.
pub fn page_for(info: &Info, https_port: u16, user_agent: &str) -> String {
    install_page(info, is_apple(user_agent)).replace("{HTTPS_PORT}", &https_port.to_string())
}

/// Headers of the helper's answers (no scripts at all on these pages).
pub const HELPER_HEADERS: [(&str, &str); 4] = [
    (
        "Content-Security-Policy",
        "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Frame-Options", "DENY"),
];

// ----- the listeners ----------------------------------------------------------------------------

/// The server certificate in use; a renewal swaps it without dropping phones.
#[derive(Debug)]
pub struct Resolver {
    current: RwLock<Arc<rustls::sign::CertifiedKey>>,
}

fn certified(m: &Material) -> Result<Arc<rustls::sign::CertifiedKey>, String> {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(m.key.to_vec()));
    let signer = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| format!("the server key cannot be used: {e}"))?;
    Ok(Arc::new(rustls::sign::CertifiedKey::new(
        vec![CertificateDer::from(m.cert_der.clone())],
        signer,
    )))
}

impl Resolver {
    pub fn new(m: &Material) -> Result<Self, String> {
        Ok(Self {
            current: RwLock::new(certified(m)?),
        })
    }
    pub fn set(&self, m: &Material) -> Result<(), String> {
        let c = certified(m)?;
        if let Ok(mut cur) = self.current.write() {
            *cur = c;
        }
        Ok(())
    }
}

impl rustls::server::ResolvesServerCert for Resolver {
    fn resolve(
        &self,
        _hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.current.read().ok().map(|c| c.clone())
    }
}

/// The TLS settings of the Wi-Fi listener.
pub fn server_config(resolver: Arc<Resolver>) -> Result<Arc<rustls::ServerConfig>, String> {
    let mut c = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_cert_resolver(resolver);
    c.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(c))
}

/// One phone connection on the Wi-Fi listener, by the relay's local port on the
/// loopback connection to the request handler.
#[derive(Debug)]
pub struct Peer {
    /// The phone's address (so failure counting sees the phone, not the relay).
    pub ip: IpAddr,
    /// The phone's side of the connection, to close it.
    stream: TcpStream,
    /// Close the connection at this time (security-review-1 A4: an unauthenticated
    /// request's body must arrive within [`UNAUTH_BODY_TIMEOUT`]).
    deadline: Option<Instant>,
}

/// The phone connections being relayed.
pub type Peers = Arc<Mutex<HashMap<u16, Peer>>>;

/// How long an unauthenticated request may take to send its body (A4).
pub const UNAUTH_BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// Unauthenticated requests handled at once, in all (A4).
pub const MAX_UNAUTH: usize = 4;
/// Unauthenticated requests handled at once from one address (A4).
pub const MAX_UNAUTH_PER_ADDRESS: usize = 2;
/// The TLS handshake must be done in this time.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The phone's address on the connection relayed from `port`.
pub fn peer_ip(peers: &Peers, port: u16) -> Option<IpAddr> {
    peers.lock().ok().and_then(|m| m.get(&port).map(|p| p.ip))
}

/// Close the connection relayed from `port` now.
pub fn close_peer(peers: &Peers, port: u16) {
    if let Ok(m) = peers.lock()
        && let Some(p) = m.get(&port)
    {
        let _ = p.stream.shutdown(Shutdown::Both);
    }
}

/// Close the connection relayed from `port` at `deadline` (or never, with `None`).
pub fn set_deadline(peers: &Peers, port: u16, deadline: Option<Instant>) {
    if let Ok(mut m) = peers.lock()
        && let Some(p) = m.get_mut(&port)
    {
        p.deadline = deadline;
    }
}

/// Close the connections whose deadline has passed; how many were closed.
pub fn sweep(peers: &Peers) -> usize {
    let now = Instant::now();
    let Ok(mut m) = peers.lock() else {
        return 0;
    };
    let mut n = 0;
    for p in m.values_mut() {
        if p.deadline.is_some_and(|d| now >= d) {
            let _ = p.stream.shutdown(Shutdown::Both);
            p.deadline = None;
            n += 1;
        }
    }
    n
}

/// Admits unauthenticated requests (A4): at most [`MAX_UNAUTH`] at a time, and
/// [`MAX_UNAUTH_PER_ADDRESS`] from one address.
#[derive(Debug, Default)]
pub struct Gate {
    open: Mutex<HashMap<IpAddr, usize>>,
}

/// An admitted unauthenticated request; its place is given back when dropped.
#[derive(Debug)]
pub struct Ticket {
    gate: Arc<Gate>,
    ip: IpAddr,
}

impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A place for a request from `ip`, or `None` when the limits are reached.
    pub fn admit(self: &Arc<Self>, ip: IpAddr) -> Option<Ticket> {
        let mut m = self.open.lock().ok()?;
        let total: usize = m.values().sum();
        let mine = m.get(&ip).copied().unwrap_or(0);
        if total >= MAX_UNAUTH || mine >= MAX_UNAUTH_PER_ADDRESS {
            return None;
        }
        m.insert(ip, mine + 1);
        Some(Ticket {
            gate: self.clone(),
            ip,
        })
    }

    /// Requests admitted now.
    pub fn open(&self) -> usize {
        self.open.lock().map(|m| m.values().sum()).unwrap_or(0)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Ok(mut m) = self.gate.open.lock()
            && let Some(n) = m.get_mut(&self.ip)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

/// Accept TLS connections until `stopping` or `gone` (L8: the address went away and
/// the listener is bound again when it is back); each is relayed to `backend`.
pub fn accept_loop(
    listener: TcpListener,
    config: Arc<rustls::ServerConfig>,
    backend: SocketAddr,
    peers: Peers,
    stopping: Arc<AtomicBool>,
    gone: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stopping.load(Ordering::SeqCst) && !gone.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((tcp, peer)) => {
                if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    drop(tcp);
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let (config, peers, act) = (config.clone(), peers.clone(), active.clone());
                let spawned = std::thread::Builder::new()
                    .name("capture-tls".into())
                    .spawn(move || {
                        let _ = relay(tcp, peer, &config, backend, &peers);
                        act.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

type Shared = Arc<Mutex<(rustls::ServerConnection, TcpStream)>>;

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// Send what the TLS connection wants to write.
fn flush(conn: &mut rustls::ServerConnection, tcp: &mut TcpStream) -> std::io::Result<()> {
    while conn.wants_write() {
        conn.write_tls(tcp)?;
    }
    tcp.flush()
}

/// One phone connection: the TLS handshake, then bytes both ways between the phone
/// and the request handler until either side closes.
fn relay(
    tcp: TcpStream,
    peer: SocketAddr,
    config: &Arc<rustls::ServerConfig>,
    backend: SocketAddr,
    peers: &Peers,
) -> std::io::Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    tcp.set_write_timeout(Some(IDLE))?;
    let _ = tcp.set_nodelay(true);
    let mut conn = rustls::ServerConnection::new(config.clone()).map_err(io_err)?;
    let mut t = tcp.try_clone()?;
    let started = Instant::now();
    while conn.is_handshaking() {
        if started.elapsed() >= HANDSHAKE_TIMEOUT {
            return Err(io_err("the TLS handshake took too long"));
        }
        conn.complete_io(&mut t)?;
    }
    tcp.set_read_timeout(Some(IDLE))?;
    let inner = TcpStream::connect_timeout(&backend, Duration::from_secs(5))?;
    inner.set_read_timeout(Some(IDLE))?;
    let port = inner.local_addr()?.port();
    if let Ok(mut p) = peers.lock() {
        p.insert(
            port,
            Peer {
                ip: peer.ip(),
                stream: tcp.try_clone()?,
                // A4: a request must arrive (headers, and a small body, which the
                // HTTP server reads itself) within the unauthenticated timeout.
                deadline: Some(Instant::now() + UNAUTH_BODY_TIMEOUT),
            },
        );
    }
    // Data the phone sent with the end of the handshake is already decrypted.
    let mut early: Vec<u8> = Vec::new();
    {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match conn.reader().read(&mut buf) {
                Ok(0) => break,
                Ok(k) => early.extend_from_slice(buf.get(..k).unwrap_or(&[])),
                Err(_) => break,
            }
        }
    }
    let shared: Shared = Arc::new(Mutex::new((conn, tcp.try_clone()?)));
    let back = {
        let shared = shared.clone();
        let mut from = inner.try_clone()?;
        let phone = tcp.try_clone()?;
        std::thread::Builder::new()
            .name("capture-tls-out".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match from.read(&mut buf) {
                        Ok(0) | Err(_) => 0,
                        Ok(n) => n,
                    };
                    let Ok(mut g) = shared.lock() else { break };
                    let (c, w) = &mut *g;
                    if n == 0 {
                        c.send_close_notify();
                        let _ = flush(c, w);
                        let _ = phone.shutdown(Shutdown::Write);
                        break;
                    }
                    let data = buf.get(..n).unwrap_or(&[]);
                    if c.writer().write_all(data).is_err() || flush(c, w).is_err() {
                        break;
                    }
                }
            })?
    };
    let mut to = inner.try_clone()?;
    if !early.is_empty() {
        to.write_all(&early)?;
    }
    let mut raw = vec![0u8; 64 * 1024];
    let mut plain = vec![0u8; 64 * 1024];
    let result = (|| -> std::io::Result<()> {
        loop {
            let n = t.read(&mut raw)?;
            if n == 0 {
                return Ok(());
            }
            let mut rd = raw.get(..n).unwrap_or(&[]);
            let mut out: Vec<u8> = Vec::new();
            let mut closed = false;
            {
                let Ok(mut g) = shared.lock() else {
                    return Ok(());
                };
                let (c, w) = &mut *g;
                while !rd.is_empty() {
                    c.read_tls(&mut rd)?;
                    let state = match c.process_new_packets() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = flush(c, w);
                            return Err(io_err(e));
                        }
                    };
                    closed |= state.peer_has_closed();
                    loop {
                        match c.reader().read(&mut plain) {
                            Ok(0) => break,
                            Ok(k) => out.extend_from_slice(plain.get(..k).unwrap_or(&[])),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }
                }
                flush(c, w)?;
            }
            if !out.is_empty() {
                to.write_all(&out)?;
            }
            if closed {
                return Ok(());
            }
        }
    })();
    let _ = inner.shutdown(Shutdown::Both);
    let _ = back.join();
    let _ = tcp.shutdown(Shutdown::Both);
    if let Ok(mut p) = peers.lock() {
        p.remove(&port);
    }
    result
}

// ----- a client for checks and tests ------------------------------------------------------------

/// `GET path` over HTTPS at `addr`, trusting only `ca_der`, for `host` (an IP address
/// or a name): the status code and body. Used by `wifi check` and the tests.
pub fn https_get(
    addr: SocketAddr,
    host: &str,
    ca_der: &[u8],
    path: &str,
) -> Result<(u16, Vec<u8>), String> {
    https_call(addr, host, ca_der, "GET", path, &[], None)
}

/// A request over HTTPS at `addr`, trusting only `ca_der`, for `host`: the status
/// code and body.
pub fn https_call(
    addr: SocketAddr,
    host: &str,
    ca_der: &[u8],
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>), String> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca_der.to_vec()))
        .map_err(|e| format!("the CA certificate cannot be used: {e}"))?;
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
    let mut conn =
        rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    let mut bytes = req.into_bytes();
    bytes.extend_from_slice(body.unwrap_or(&[]));
    tls.write_all(&bytes).map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    match tls.read_to_end(&mut answer) {
        Ok(_) => {}
        Err(e) if !answer.is_empty() && e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(e) => return Err(e.to_string()),
    }
    parse_answer(&answer)
}

/// The status and body of an HTTP/1.1 answer (`Connection: close`; chunked bodies
/// are joined).
pub fn parse_answer(answer: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(answer.get(..split).unwrap_or(&[])).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no HTTP status")?;
    let body = answer.get(split + 4..).unwrap_or(&[]).to_vec();
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    });
    if !chunked {
        return Ok((status, body));
    }
    let mut out = Vec::new();
    let mut rest: &[u8] = &body;
    while let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(
            String::from_utf8_lossy(rest.get(..eol).unwrap_or(&[])).trim(),
            16,
        )
        .unwrap_or(0);
        if size == 0 {
            break;
        }
        out.extend_from_slice(rest.get(eol + 2..eol + 2 + size).unwrap_or(&[]));
        rest = rest.get(eol + 4 + size..).unwrap_or(&[]);
    }
    Ok((status, out))
}

/// A plain-HTTP answer: status, headers (lower-case names) and body.
pub type HttpAnswer = (u16, Vec<(String, String)>, Vec<u8>);

/// `GET path` in plain HTTP (the helper port): the whole answer.
pub fn http_get(addr: SocketAddr, path: &str, user_agent: &str) -> Result<HttpAnswer, String> {
    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: {user_agent}\r\nConnection: close\r\n\r\n"
    );
    tcp.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    let _ = tcp.read_to_end(&mut answer);
    let split = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(answer.get(..split).unwrap_or(&[])).into_owned();
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let (status, body) = parse_answer(&answer)?;
    Ok((status, headers, body))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn names_addresses_and_fingerprints() {
        assert_eq!(check_name("").unwrap(), None);
        assert_eq!(
            check_name("MoneyKeep.Home.Arpa.").unwrap().as_deref(),
            Some("moneykeep.home.arpa")
        );
        assert!(check_name("no spaces.arpa").is_err());
        assert!(check_name("single").is_err());
        assert!(check_name("10.0.0.1").is_err());
        assert!(check_address("192.168.1.20").is_ok());
        assert!(check_address("8.8.8.8").is_err());
        assert!(check_address("127.0.0.1").is_ok());
        assert!(check_address("fe80::1").is_err());
        let fp = fingerprint(b"abc");
        assert_eq!(fp.split(' ').count(), 8);
        assert!(fp.split(' ').all(|g| g.len() == 4));
        assert_eq!(clean_computer("  DESK<TOP>-1 \n"), "DESKTOP-1");
        assert_eq!(clean_computer(""), "this computer");
        let u = uuid([0xab; 16]);
        assert_eq!(u.len(), 36);
        assert_eq!(u.get(14..15), Some("4"));
    }

    #[test]
    fn the_profile_and_the_page() {
        let target = Target {
            ip: "192.168.1.20".parse().unwrap(),
            name: None,
        };
        let (der, _) = new_ca("MoneyKeep Capture CA (PC)", &target, 1_800_000_000).unwrap();
        let p = mobileconfig(&der, "MoneyKeep Capture CA (PC)");
        assert!(p.contains("<string>com.apple.security.root</string>"));
        assert!(p.contains(&base64::engine::general_purpose::STANDARD.encode(&der)));
        assert_eq!(p, mobileconfig(&der, "MoneyKeep Capture CA (PC)"));
        let info = Info {
            ca_name: "MoneyKeep Capture CA (PC)".into(),
            address: "192.168.1.20".into(),
            fingerprint: fingerprint(&der),
            ..Info::default()
        };
        let android = page_for(&info, 14443, "Mozilla/5.0 (Linux; Android 14)");
        let iphone = page_for(&info, 14443, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0)");
        for page in [&android, &iphone] {
            assert!(page.contains(&info.fingerprint));
            assert!(page.contains("href=\"/ca.crt\""));
            assert!(page.contains("href=\"/moneykeep-ca.mobileconfig\""));
            assert!(page.contains("https://192.168.1.20:14443"));
            assert!(!page.contains("<script"));
        }
        let a = android.find("Android phone or tablet").unwrap();
        let i = android.find("iPhone or iPad</h2>").unwrap();
        assert!(a < i, "Android first for an Android phone");
        let a = iphone.find("Android phone or tablet").unwrap();
        let i = iphone.find("iPhone or iPad</h2>").unwrap();
        assert!(i < a, "iPhone first for an iPhone");
        assert!(iphone.contains("Certificate Trust Settings"));
        assert!(android.contains("Install a certificate"));
    }
}
