//! Native decryption of encrypted iOS (iTunes/Finder) backups.
//!
//! An encrypted backup keeps its `Manifest.db` (and every file blob) AES-CBC
//! encrypted; only `Manifest.plist` stays plaintext, carrying the `BackupKeyBag`
//! and the wrapped `ManifestKey`. The decrypt path is a fixed ladder:
//!
//! ```text
//!   k0        = PBKDF2-HMAC-SHA256(password, DPSL, DPIC)
//!   KEK       = PBKDF2-HMAC-SHA1 (k0,       SALT, ITER, dklen=32)
//!   class_key = AES-unwrap(KEK,       keybag WPKY)          (RFC 3394)
//!   file_key  = AES-unwrap(class_key, EncryptionKey[4:])
//!   plaintext = AES-CBC(file_key, IV=0, ciphertext)[:Size]
//! ```
//!
//! (`ManifestKey` unwraps the same way, then decrypts `Manifest.db`.) The 4-byte
//! little-endian prefix on `ManifestKey`/`EncryptionKey` selects the protection
//! class whose key does the unwrap. AES key-unwrap has a built-in integrity
//! check, so a wrong password surfaces as an unwrap failure — that's how we
//! validate the password without a separate check.
//!
//! provenance: reference (own implementation) of the iTunes-backup keybag format
//! as parsed by iLEAPP's `scripts/search_files.py` and the `iOSbackup` library.
//! This is the Phase-2 native Decryptor; the MVP still lets iLEAPP decrypt the
//! artifacts it parses, but native readers (camera roll) need plaintext files.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use aes::Aes256;
use aes_kw::KekAes256;
use cbc::cipher::block_padding::NoPadding;
use cbc::cipher::generic_array::GenericArray;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use pbkdf2::pbkdf2_hmac_array;
use sha1::Sha1;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::{Error, Result};

/// Backups always encrypt with an all-zero IV; the real length is recovered
/// from the file's `Size`, and Manifest.db is page-aligned (multiple of 16).
const ZERO_IV: [u8; 16] = [0u8; 16];

/// Keybag WRAP bit: the class key is wrapped under the passcode-derived KEK.
/// (Bit 1, WRAP_DEVICE, needs the device UID key we don't have from a password.)
const WRAP_PASSCODE: u32 = 2;

fn err(msg: impl Into<String>) -> Error {
    Error::Decrypt(msg.into())
}

/// One protection class's wrapped key from the keybag.
struct ClassEntry {
    wrap: u32,
    wpky: Vec<u8>,
}

/// The parsed `BackupKeyBag`: KDF parameters plus each protection class's
/// wrapped key. Enough to derive the KEK and unwrap every class key.
struct Keybag {
    salt: Vec<u8>,
    iter: u32,
    dpsl: Vec<u8>,
    dpic: u32,
    classes: HashMap<u32, ClassEntry>,
}

impl Keybag {
    /// Parse the TLV blob: 4-byte tag, 4-byte big-endian length, value. Global
    /// KDF fields precede the first class; each class record opens with a fresh
    /// `UUID` and carries `CLAS`/`WRAP`/`WPKY`.
    fn parse(blob: &[u8]) -> Result<Self> {
        let mut salt = None;
        let mut iter = None;
        let mut dpsl = None;
        let mut dpic = None;
        let mut classes = HashMap::new();

        // The current class being accumulated (None until the first class UUID).
        let mut cur_clas: Option<u32> = None;
        let mut cur_wrap: u32 = 0;
        let mut cur_wpky: Vec<u8> = Vec::new();
        let mut uuid_seen = false;

        let mut flush = |clas: &mut Option<u32>, wrap: &mut u32, wpky: &mut Vec<u8>| {
            if let Some(c) = clas.take() {
                classes.insert(
                    c,
                    ClassEntry {
                        wrap: *wrap,
                        wpky: std::mem::take(wpky),
                    },
                );
                *wrap = 0;
            }
        };

        let mut i = 0usize;
        while i + 8 <= blob.len() {
            let tag = &blob[i..i + 4];
            let len = u32::from_be_bytes(blob[i + 4..i + 8].try_into().unwrap()) as usize;
            i += 8;
            if i + len > blob.len() {
                return Err(err("truncated keybag TLV"));
            }
            let val = &blob[i..i + len];
            i += len;

            match tag {
                b"UUID" => {
                    if !uuid_seen {
                        // The keybag's own UUID (global header), not a class.
                        uuid_seen = true;
                    } else {
                        // A new protection-class record begins.
                        flush(&mut cur_clas, &mut cur_wrap, &mut cur_wpky);
                        cur_clas = Some(0);
                    }
                }
                b"CLAS" if cur_clas.is_some() => cur_clas = Some(be_u32(val)?),
                // A WRAP before the first class is the global one (falls through
                // to `_` and is ignored); inside a class it's that class's mode.
                b"WRAP" if cur_clas.is_some() => cur_wrap = be_u32(val)?,
                b"WPKY" if cur_clas.is_some() => cur_wpky = val.to_vec(),
                b"SALT" => salt = Some(val.to_vec()),
                b"ITER" => iter = Some(be_u32(val)?),
                b"DPSL" => dpsl = Some(val.to_vec()),
                b"DPIC" => dpic = Some(be_u32(val)?),
                _ => {} // VERS, TYPE, HMCK, KTYP, PBKY, … not needed here.
            }
        }
        flush(&mut cur_clas, &mut cur_wrap, &mut cur_wpky);

        Ok(Keybag {
            salt: salt.ok_or_else(|| err("keybag missing SALT"))?,
            iter: iter.ok_or_else(|| err("keybag missing ITER"))?,
            dpsl: dpsl.ok_or_else(|| err("keybag missing DPSL (unsupported/old backup)"))?,
            dpic: dpic.ok_or_else(|| err("keybag missing DPIC (unsupported/old backup)"))?,
            classes,
        })
    }

    /// Derive the passcode key (KEK) via the two-stage PBKDF2 modern iOS uses.
    fn derive_kek(&self, password: &[u8]) -> [u8; 32] {
        // Zero the intermediate key when it drops; the returned KEK is wrapped by
        // the caller.
        let k0 = Zeroizing::new(pbkdf2_hmac_array::<Sha256, 32>(
            password, &self.dpsl, self.dpic,
        ));
        pbkdf2_hmac_array::<Sha1, 32>(&*k0, &self.salt, self.iter)
    }

    /// Unwrap every passcode-wrapped class key with the KEK. A single malformed
    /// class entry is skipped rather than failing the whole backup, but a wrong
    /// password makes *every* RFC-3394 integrity check fail, leaving the map
    /// empty — which we report as an error. So a wrong password never yields bad
    /// keys, while one corrupt class doesn't reject a correct password.
    fn class_keys(&self, kek: &[u8; 32]) -> Result<HashMap<u32, [u8; 32]>> {
        let kek = KekAes256::from(*kek);
        let mut out = HashMap::new();
        for (clas, entry) in &self.classes {
            if entry.wrap & WRAP_PASSCODE == 0 {
                continue; // device-only class key; not recoverable from a password.
            }
            // Unwrap failure (wrong KEK, or a stray/short WPKY) → skip this class.
            if let Ok(unwrapped) = kek.unwrap_vec(&entry.wpky) {
                if let Ok(key) = <[u8; 32]>::try_from(unwrapped) {
                    out.insert(*clas, key);
                }
            }
        }
        if out.is_empty() {
            // No class unwrapped: wrong password (or an unsupported keybag).
            return Err(err("wrong password (no class key could be unwrapped)"));
        }
        Ok(out)
    }
}

/// Opens an encrypted backup's keys once, then decrypts `Manifest.db` and
/// individual files on demand. Cheap to hold; carries only unwrapped keys.
pub struct BackupDecryptor {
    backup_dir: PathBuf,
    class_keys: HashMap<u32, [u8; 32]>,
    manifest_key: [u8; 32],
}

impl Drop for BackupDecryptor {
    /// Zero the unwrapped key material when the decryptor drops, so it doesn't
    /// linger in freed memory for the rest of the process.
    fn drop(&mut self) {
        self.manifest_key.zeroize();
        for key in self.class_keys.values_mut() {
            key.zeroize();
        }
    }
}

impl BackupDecryptor {
    /// Read `Manifest.plist`, parse the keybag, derive keys from `password`, and
    /// unwrap the manifest key. Errors (as `Error::Decrypt`) on a wrong password.
    pub fn open(backup_dir: &Path, password: &str) -> Result<Self> {
        let plist_path = backup_dir.join("Manifest.plist");
        let root = plist::Value::from_file(&plist_path).map_err(|source| Error::Plist {
            path: plist_path.clone(),
            source,
        })?;
        let dict = root
            .as_dictionary()
            .ok_or_else(|| err("Manifest.plist is not a dictionary"))?;

        let keybag_blob = dict
            .get("BackupKeyBag")
            .and_then(|v| v.as_data())
            .ok_or_else(|| err("Manifest.plist has no BackupKeyBag (not an encrypted backup?)"))?;
        let manifest_key_field = dict
            .get("ManifestKey")
            .and_then(|v| v.as_data())
            .ok_or_else(|| err("Manifest.plist has no ManifestKey"))?;

        let keybag = Keybag::parse(keybag_blob)?;
        let kek = Zeroizing::new(keybag.derive_kek(password.as_bytes()));
        let mut class_keys = keybag.class_keys(&kek)?;
        // On the happy path BackupDecryptor::Drop zeroizes class_keys; on this
        // early error it would drop un-zeroized, so wipe the keys ourselves first.
        let manifest_key = match unwrap_prefixed(&class_keys, manifest_key_field) {
            Ok(k) => k,
            Err(e) => {
                for v in class_keys.values_mut() {
                    v.zeroize();
                }
                return Err(e);
            }
        };

        Ok(BackupDecryptor {
            backup_dir: backup_dir.to_path_buf(),
            class_keys,
            manifest_key,
        })
    }

    /// Decrypt `Manifest.db` to plaintext SQLite bytes. Page-aligned, so no
    /// length fixup is needed.
    pub fn decrypt_manifest_db(&self) -> Result<Vec<u8>> {
        let path = self.backup_dir.join("Manifest.db");
        let ct = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        aes_cbc_decrypt(&self.manifest_key, &ct)
    }

    /// Decrypt one backed-up file. `file_blob` is the `Files.file` column from
    /// Manifest.db (carrying the wrapped per-file key and real size); `file_id`
    /// locates the ciphertext at `<backup>/<id[:2]>/<id>`.
    pub fn decrypt_file(&self, file_blob: &[u8], file_id: &str) -> Result<Vec<u8>> {
        // `file_id` is from the untrusted Manifest; reject anything that isn't a
        // content-addressed hex id so it can't `join` its way out of backup_dir.
        if !is_valid_file_id(file_id) {
            return Err(err("invalid file id"));
        }
        let (enc_key, size) = file_key_field(file_blob)?;
        let ct_path = self.backup_dir.join(&file_id[..2]).join(file_id);
        let ct = std::fs::read(&ct_path).map_err(|e| Error::io(&ct_path, e))?;
        self.decrypt_bytes(&enc_key, &ct, size.and_then(|s| usize::try_from(s).ok()))
    }

    /// Decrypt raw ciphertext given a file's class-prefixed wrapped key (the
    /// `EncryptionKey` field, from `file_key_field`). This is the on-demand
    /// primitive the media layer uses: the wrapped key is stored on the cache
    /// row, so a single photo decrypts without re-reading Manifest.db. `size`
    /// trims the CBC block padding back to the real plaintext length.
    pub fn decrypt_bytes(
        &self,
        enc_key_field: &[u8],
        ciphertext: &[u8],
        size: Option<usize>,
    ) -> Result<Vec<u8>> {
        // Zeroizing so the file key doesn't linger on the stack after decryption.
        let key = Zeroizing::new(unwrap_prefixed(&self.class_keys, enc_key_field)?);
        let mut pt = aes_cbc_decrypt(&key, ciphertext)?;
        if let Some(size) = size {
            // Trim the CBC block padding back to the real plaintext length. A wrong
            // key can't reach here as an over-long size: it yields garbage of the
            // exact ciphertext length, so `size <= pt.len()` always holds for a
            // valid record. A `size` beyond the buffer therefore only means bad
            // `Size` metadata — clamp (serve untrimmed, ≤15 bytes of harmless
            // trailing padding for a DB/media file) rather than failing the read.
            pt.truncate(size.min(pt.len()));
        }
        Ok(pt)
    }
}

/// Extract a Manifest.db `file` blob's class-prefixed wrapped key and plaintext
/// size. The wrapped key can be stored per-file (it's useless without the class
/// keys) so a photo can be decrypted on demand later via [`BackupDecryptor::decrypt_bytes`].
/// Whether `id` is a content-addressed backup file id safe to use as a path
/// component. Backup file ids are 40-char SHA-1 hex; requiring hex rejects the
/// `..`, `/`, and absolute-path values a malicious Manifest could carry (which
/// would otherwise escape `backup_dir` via `Path::join`).
pub fn is_valid_file_id(id: &str) -> bool {
    (2..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn file_key_field(blob: &[u8]) -> Result<(Vec<u8>, Option<u64>)> {
    let meta = FileMeta::parse(blob)?;
    Ok((meta.enc_key, meta.size))
}

/// The bits of a Manifest.db `file` blob we need: the (class-prefixed) wrapped
/// key and the real plaintext size.
struct FileMeta {
    enc_key: Vec<u8>,
    size: Option<u64>,
}

impl FileMeta {
    fn parse(blob: &[u8]) -> Result<Self> {
        let val = plist::Value::from_reader(Cursor::new(blob))
            .map_err(|_| err("malformed file-metadata plist"))?;
        let dict = val
            .as_dictionary()
            .ok_or_else(|| err("file metadata is not a dictionary"))?;
        // Real backups NSKeyedArchive the MBFile record (a `$objects` graph);
        // our test fixtures use the flattened form iLEAPP's search_files.py reads.
        if dict.contains_key("$objects") {
            Self::parse_archived(dict)
        } else {
            Self::parse_flat(dict)
        }
    }

    /// Flattened form: `{ EncryptionKey: { NS.data: <field> }, Size: <int> }`.
    fn parse_flat(dict: &plist::Dictionary) -> Result<Self> {
        let enc_key = dict
            .get("EncryptionKey")
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("NS.data"))
            .and_then(|v| v.as_data())
            .ok_or_else(|| err("file metadata has no EncryptionKey/NS.data"))?
            .to_vec();
        Ok(FileMeta {
            enc_key,
            size: dict.get("Size").and_then(plist_int),
        })
    }

    /// NSKeyedArchiver form (real backups): resolve `$top.root` into `$objects`,
    /// follow the `EncryptionKey` UID to its `NS.data`, read the inline `Size`.
    /// Untested against a real device backup yet — see module note.
    fn parse_archived(dict: &plist::Dictionary) -> Result<Self> {
        let objects = dict
            .get("$objects")
            .and_then(|v| v.as_array())
            .ok_or_else(|| err("archived metadata has no $objects"))?;
        let obj = |idx: usize| objects.get(idx).and_then(|v| v.as_dictionary());

        let root_uid = dict
            .get("$top")
            .and_then(|v| v.as_dictionary())
            .and_then(|t| t.get("root"))
            .and_then(|v| v.as_uid())
            .map(|u| u.get() as usize)
            .ok_or_else(|| err("archived metadata has no $top.root"))?;
        let root = obj(root_uid).ok_or_else(|| err("archived root object missing"))?;

        let enc_uid = root
            .get("EncryptionKey")
            .and_then(|v| v.as_uid())
            .map(|u| u.get() as usize)
            .ok_or_else(|| err("archived file has no EncryptionKey"))?;
        let enc_key = obj(enc_uid)
            .and_then(|d| d.get("NS.data"))
            .and_then(|v| v.as_data())
            .ok_or_else(|| err("EncryptionKey object has no NS.data"))?
            .to_vec();

        // `Size` is usually an inline integer, but resolve a UID reference too
        // (NSKeyedArchiver can box it). An unreadable Size → None → the file is
        // served untrimmed rather than mis-truncated.
        Ok(FileMeta {
            enc_key,
            size: root
                .get("Size")
                .and_then(|v| resolve_archived_int(v, objects)),
        })
    }
}

/// Unwrap a class-prefixed key field: 4-byte little-endian protection class,
/// then the RFC-3394-wrapped 32-byte key.
fn unwrap_prefixed(class_keys: &HashMap<u32, [u8; 32]>, field: &[u8]) -> Result<[u8; 32]> {
    if field.len() < 4 + 8 {
        return Err(err("wrapped-key field too short"));
    }
    let clas = u32::from_le_bytes(field[0..4].try_into().unwrap());
    let class_key = class_keys
        .get(&clas)
        .ok_or_else(|| err(format!("no key for protection class {clas}")))?;
    // The unwrapped bytes are a plaintext class/file key — zeroize the heap buffer
    // once it's copied into the fixed array.
    let unwrapped = Zeroizing::new(
        KekAes256::from(*class_key)
            .unwrap_vec(&field[4..])
            .map_err(|_| err("key unwrap failed (wrong password or corrupt keybag)"))?,
    );
    <[u8; 32]>::try_from(unwrapped.as_slice()).map_err(|_| err("unexpected unwrapped-key length"))
}

/// AES-256-CBC decrypt with a zero IV. `ct` must be a positive multiple of 16.
fn aes_cbc_decrypt(key: &[u8; 32], ct: &[u8]) -> Result<Vec<u8>> {
    if ct.is_empty() || !ct.len().is_multiple_of(16) {
        return Err(err("ciphertext length is not a positive multiple of 16"));
    }
    // Decrypt the whole buffer in ONE call. A per-block loop
    // (`decrypt_block_mut` over `chunks_exact_mut(16)`) forces the AES backend
    // to process one 16-byte block at a time, which defeats hardware AES
    // pipelining (~8 blocks at once) and runs ~50x slower — decrypting the two
    // ~433 MB backup DBs and every thumbnail that way was the multi-minute
    // camera-roll import bottleneck. `NoPadding` on a block-aligned buffer
    // removes nothing, so the plaintext is byte-identical to the old path.
    let mut buf = ct.to_vec();
    let dec = cbc::Decryptor::<Aes256>::new(
        GenericArray::from_slice(key),
        GenericArray::from_slice(&ZERO_IV),
    );
    let n = dec
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| err("AES-CBC decrypt failed"))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

fn be_u32(val: &[u8]) -> Result<u32> {
    val.get(..4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or_else(|| err("keybag integer field too short"))
}

/// A plist integer that may be encoded signed or unsigned. Rejects negatives: a
/// file `Size` is never negative, and a negative read as `u64` would become a
/// ~1.8e19 value that then trips the "size exceeds decrypted length" guard.
fn plist_int(v: &plist::Value) -> Option<u64> {
    if let Some(u) = v.as_unsigned_integer() {
        return Some(u);
    }
    v.as_signed_integer().filter(|i| *i >= 0).map(|i| i as u64)
}

/// Resolve an archived integer field that may be stored inline, or (in an
/// NSKeyedArchiver graph) as a UID reference to a boxed number in `$objects`.
/// Returns None when it can't be confidently read — the caller then leaves the
/// plaintext untrimmed (harmless trailing block padding) rather than truncating
/// to a wrong length.
fn resolve_archived_int(v: &plist::Value, objects: &[plist::Value]) -> Option<u64> {
    if let Some(n) = plist_int(v) {
        return Some(n);
    }
    let idx = v.as_uid()?.get() as usize;
    objects.get(idx).and_then(plist_int)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::Aes256;
    use cbc::cipher::BlockEncryptMut;

    const PW: &str = "traceloupe-test";
    const CLASS_ID: u32 = 3;
    const DPIC: u32 = 1000; // low iteration counts keep the test fast.
    const ITER: u32 = 1000;

    fn aes_cbc_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
        let mut buf = plaintext.to_vec();
        if !buf.len().is_multiple_of(16) {
            buf.resize(buf.len() + (16 - buf.len() % 16), 0);
        }
        let mut enc = cbc::Encryptor::<Aes256>::new(
            GenericArray::from_slice(key),
            GenericArray::from_slice(&ZERO_IV),
        );
        for chunk in buf.chunks_exact_mut(16) {
            enc.encrypt_block_mut(GenericArray::from_mut_slice(chunk));
        }
        buf
    }

    #[test]
    #[ignore] // perf probe: `cargo test -p traceloupe-core --lib -- --ignored --nocapture aes_cbc_throughput`
    fn aes_cbc_throughput() {
        let key = [0x11u8; 32];
        let mb = 128usize;
        let plaintext = vec![0xABu8; mb * 1024 * 1024];
        let ct = aes_cbc_encrypt(&key, &plaintext);
        let t = std::time::Instant::now();
        let pt = super::aes_cbc_decrypt(&key, &ct).unwrap();
        let secs = t.elapsed().as_secs_f64();
        eprintln!(
            "aes_cbc_decrypt: {mb} MB in {secs:.2}s = {:.0} MB/s",
            mb as f64 / secs
        );
        #[cfg(target_arch = "aarch64")]
        eprintln!(
            "aarch64 hardware AES available: {}",
            std::arch::is_aarch64_feature_detected!("aes")
        );
        assert_eq!(pt.len(), plaintext.len());
    }

    fn tlv(tag: &[u8; 4], value: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(tag);
        v.extend_from_slice(&(value.len() as u32).to_be_bytes());
        v.extend_from_slice(value);
        v
    }

    /// Build a keybag + Manifest.plist + encrypted Manifest.db + one encrypted
    /// file, exactly as `tools/make_fixture_backup.py` does, in a temp dir.
    /// Returns (dir, manifest_plaintext, file_blob, file_id, file_plaintext).
    fn make_fixture() -> (tempfile::TempDir, Vec<u8>, Vec<u8>, String, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path();

        let salt = [7u8; 20];
        let dpsl = [9u8; 32];
        let k0 = pbkdf2_hmac_array::<Sha256, 32>(PW.as_bytes(), &dpsl, DPIC);
        let kek = pbkdf2_hmac_array::<Sha1, 32>(&k0, &salt, ITER);

        let class_key = [0x11u8; 32];
        let class_wpky = KekAes256::from(kek).wrap_vec(&class_key).unwrap();

        // Keybag: global header + KDF params, then one class record.
        let mut kb = Vec::new();
        kb.extend(tlv(b"VERS", &3u32.to_be_bytes()));
        kb.extend(tlv(b"TYPE", &1u32.to_be_bytes()));
        kb.extend(tlv(b"UUID", &[1u8; 16])); // keybag UUID (global)
        kb.extend(tlv(b"HMCK", &[2u8; 40]));
        kb.extend(tlv(b"WRAP", &1u32.to_be_bytes())); // global wrap
        kb.extend(tlv(b"SALT", &salt));
        kb.extend(tlv(b"ITER", &ITER.to_be_bytes()));
        kb.extend(tlv(b"DPSL", &dpsl));
        kb.extend(tlv(b"DPIC", &DPIC.to_be_bytes()));
        kb.extend(tlv(b"UUID", &[3u8; 16])); // starts the class record
        kb.extend(tlv(b"CLAS", &CLASS_ID.to_be_bytes()));
        kb.extend(tlv(b"WRAP", &3u32.to_be_bytes())); // DEVICE|PASSCODE
        kb.extend(tlv(b"KTYP", &0u32.to_be_bytes()));
        kb.extend(tlv(b"WPKY", &class_wpky));

        // Manifest key, wrapped under the class key; field = LE(class) + wrapped.
        let manifest_key = [0x22u8; 32];
        let manifest_wrapped = KekAes256::from(class_key).wrap_vec(&manifest_key).unwrap();
        let mut manifest_key_field = CLASS_ID.to_le_bytes().to_vec();
        manifest_key_field.extend_from_slice(&manifest_wrapped);

        // A page-aligned "Manifest.db" plaintext, encrypted under the manifest key.
        let manifest_plain = vec![0xABu8; 512];
        std::fs::write(
            out.join("Manifest.db"),
            aes_cbc_encrypt(&manifest_key, &manifest_plain),
        )
        .unwrap();

        // One backed-up file: wrapped per-file key + a metadata blob + ciphertext.
        let file_plain = b"hello camera roll".to_vec(); // 17 bytes (not 16-aligned)
        let file_key = [0x33u8; 32];
        let file_wrapped = KekAes256::from(class_key).wrap_vec(&file_key).unwrap();
        let mut enc_key_field = CLASS_ID.to_le_bytes().to_vec();
        enc_key_field.extend_from_slice(&file_wrapped);

        let file_id = "aa112233445566778899aabbccddeeff00112233".to_string();
        let blob_dir = out.join(&file_id[..2]);
        std::fs::create_dir_all(&blob_dir).unwrap();
        std::fs::write(
            blob_dir.join(&file_id),
            aes_cbc_encrypt(&file_key, &file_plain),
        )
        .unwrap();

        // Flattened file-metadata plist (matches the Python fixture generator).
        let mut enc_dict = plist::Dictionary::new();
        enc_dict.insert("NS.data".into(), plist::Value::Data(enc_key_field));
        let mut meta = plist::Dictionary::new();
        meta.insert("EncryptionKey".into(), plist::Value::Dictionary(enc_dict));
        meta.insert(
            "Size".into(),
            plist::Value::Integer((file_plain.len() as i64).into()),
        );
        let mut file_blob = Vec::new();
        plist::Value::Dictionary(meta)
            .to_writer_binary(&mut file_blob)
            .unwrap();

        // Manifest.plist (plaintext) with the keybag and manifest key.
        let mut mp = plist::Dictionary::new();
        mp.insert("IsEncrypted".into(), plist::Value::Boolean(true));
        mp.insert("BackupKeyBag".into(), plist::Value::Data(kb));
        mp.insert("ManifestKey".into(), plist::Value::Data(manifest_key_field));
        plist::Value::Dictionary(mp)
            .to_file_binary(out.join("Manifest.plist"))
            .unwrap();

        (dir, manifest_plain, file_blob, file_id, file_plain)
    }

    #[test]
    fn decrypts_manifest_and_file_with_correct_password() {
        let (dir, manifest_plain, file_blob, file_id, file_plain) = make_fixture();
        let dec = BackupDecryptor::open(dir.path(), PW).unwrap();

        assert_eq!(dec.decrypt_manifest_db().unwrap(), manifest_plain);

        let got = dec.decrypt_file(&file_blob, &file_id).unwrap();
        // Size-trimmed back to the real 17 bytes, not the 32-byte padded blocks.
        assert_eq!(got, file_plain);
    }

    #[test]
    fn wrong_password_fails_to_open() {
        let (dir, ..) = make_fixture();
        // `BackupDecryptor` holds raw keys, so it intentionally isn't `Debug`;
        // match on the error rather than `unwrap_err()`.
        let result = BackupDecryptor::open(dir.path(), "not-the-password");
        assert!(
            matches!(result, Err(Error::Decrypt(_))),
            "wrong password should fail to unwrap the class key",
        );
    }

    #[test]
    fn keybag_parses_kdf_params_and_class() {
        // A minimal keybag round-trips its global params and one class entry.
        let kek = pbkdf2_hmac_array::<Sha1, 32>(
            &pbkdf2_hmac_array::<Sha256, 32>(PW.as_bytes(), &[9u8; 32], DPIC),
            &[7u8; 20],
            ITER,
        );
        let class_key = [0x11u8; 32];
        let wpky = KekAes256::from(kek).wrap_vec(&class_key).unwrap();

        let mut kb = Vec::new();
        kb.extend(tlv(b"UUID", &[1u8; 16]));
        kb.extend(tlv(b"SALT", &[7u8; 20]));
        kb.extend(tlv(b"ITER", &ITER.to_be_bytes()));
        kb.extend(tlv(b"DPSL", &[9u8; 32]));
        kb.extend(tlv(b"DPIC", &DPIC.to_be_bytes()));
        kb.extend(tlv(b"UUID", &[3u8; 16]));
        kb.extend(tlv(b"CLAS", &CLASS_ID.to_be_bytes()));
        kb.extend(tlv(b"WRAP", &2u32.to_be_bytes()));
        kb.extend(tlv(b"WPKY", &wpky));

        let parsed = Keybag::parse(&kb).unwrap();
        assert_eq!(parsed.dpic, DPIC);
        assert_eq!(parsed.iter, ITER);
        let keys = parsed.class_keys(&kek).unwrap();
        assert_eq!(keys.get(&CLASS_ID), Some(&class_key));
    }
}
