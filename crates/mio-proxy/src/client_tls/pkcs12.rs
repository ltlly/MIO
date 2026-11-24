use super::*;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) struct ZeroizeBytes(pub(super) Vec<u8>);

impl Drop for ZeroizeBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(super) struct PrivateTempFile {
    pub(super) path: PathBuf,
}

impl PrivateTempFile {
    pub(super) fn create(prefix: &str, bytes: &[u8]) -> Result<Self, ProxyError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        #[cfg(unix)]
        let base_dir = {
            let tmp = Path::new("/tmp");
            if tmp.is_dir() {
                tmp.to_path_buf()
            } else {
                std::env::temp_dir()
            }
        };
        #[cfg(not(unix))]
        let base_dir = std::env::temp_dir();
        for _ in 0..64 {
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = base_dir.join(format!("{prefix}-{pid}-{nanos}-{seq}.tmp"));
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            opts.mode(0o600);
            match opts.open(&path) {
                Ok(mut file) => {
                    let guard = Self { path };
                    file.write_all(bytes).map_err(ProxyError::tls)?;
                    file.flush().map_err(ProxyError::tls)?;
                    return Ok(guard);
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(ProxyError::Tls(format!(
                        "无法创建私有临时 PKCS#12 文件：{err}"
                    )));
                }
            }
        }
        Err(ProxyError::Tls("无法分配唯一临时 PKCS#12 文件名".into()))
    }
}

impl Drop for PrivateTempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// shortcut: shells out to macOS `/usr/bin/openssl pkcs12` to avoid unvetted pure-Rust PKCS#12 crates; upgrade only if locked workspace dependencies gain audited native PKCS#12 support.
pub(super) fn unpack_pkcs12_to_pem(
    pkcs12_bytes: &[u8],
    password: &str,
) -> Result<Vec<u8>, ProxyError> {
    let temp = PrivateTempFile::create("mio-p12", pkcs12_bytes)?;
    match run_openssl_pkcs12(&temp.path, password, false) {
        Ok(stdout) => Ok(stdout),
        Err(Pkcs12RunError::Failed { stderr }) if needs_openssl3_legacy_retry(&stderr) => {
            match run_openssl_pkcs12(&temp.path, password, true) {
                Ok(stdout) => Ok(stdout),
                Err(Pkcs12RunError::Failed {
                    stderr: retry_stderr,
                }) => Err(classify_pkcs12_stderr(&retry_stderr)),
                #[cfg(windows)]
                Err(Pkcs12RunError::NotFound) => unpack_pkcs12_windows_native(pkcs12_bytes, password),
                Err(Pkcs12RunError::Fatal(error)) => Err(error),
            }
        }
        Err(Pkcs12RunError::Failed { stderr }) => Err(classify_pkcs12_stderr(&stderr)),
        #[cfg(windows)]
        Err(Pkcs12RunError::NotFound) => unpack_pkcs12_windows_native(pkcs12_bytes, password),
        Err(Pkcs12RunError::Fatal(error)) => Err(error),
    }
}

enum Pkcs12RunError {
    Failed {
        stderr: String,
    },
    #[cfg(windows)]
    NotFound,
    Fatal(ProxyError),
}

fn needs_openssl3_legacy_retry(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    (lower.contains("unsupported") || lower.contains("digital envelope routines"))
        && !lower.contains("invalid password")
        && !lower.contains("mac verify")
}

fn classify_pkcs12_stderr(stderr: &str) -> ProxyError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("invalid password") || lower.contains("mac verify") {
        ProxyError::Tls("PKCS#12 解密失败：口令错误或数据完整性校验未通过".into())
    } else {
        ProxyError::Tls("PKCS#12 解析失败：文件格式无效或加密算法不受支持".into())
    }
}

fn run_openssl_pkcs12(
    input_path: &Path,
    password: &str,
    legacy: bool,
) -> Result<Vec<u8>, Pkcs12RunError> {
    let openssl_bin = resolve_openssl_bin();
    let mut cmd = Command::new(&openssl_bin);
    cmd.arg("pkcs12")
        .arg("-in")
        .arg(input_path)
        .arg("-nodes")
        .arg("-passin")
        .arg("stdin");
    if legacy {
        cmd.arg("-legacy");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            #[cfg(windows)]
            if err.kind() == io::ErrorKind::NotFound {
                return Pkcs12RunError::NotFound;
            }
            Pkcs12RunError::Fatal(ProxyError::Tls(format!(
                "无法启动 {} 解析 PKCS#12：{err}",
                openssl_bin.to_string_lossy()
            )))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let mut pw_line = ZeroizeBytes(Vec::with_capacity(password.len() + 1));
        pw_line.0.extend_from_slice(password.as_bytes());
        pw_line.0.push(b'\n');
        // BrokenPipe can happen if openssl immediately rejects malformed input; let exit status decide.
        let _ = stdin.write_all(&pw_line.0);
    }

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| Pkcs12RunError::Fatal(ProxyError::Tls("缺少子进程 stdout 管道".into())))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| Pkcs12RunError::Fatal(ProxyError::Tls("缺少子进程 stderr 管道".into())))?;

    let stdout_reader = thread::spawn(move || read_bounded(stdout_pipe, MAX_PKCS12_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr_pipe, MAX_PKCS12_STDERR_BYTES));

    let deadline = Instant::now() + PKCS12_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Ok(Ok((mut out, _))) = stdout_reader.join() {
                        out.fill(0);
                    }
                    let _ = stderr_reader.join();
                    return Err(Pkcs12RunError::Fatal(ProxyError::Tls(
                        "PKCS#12 解析超过 5 秒期限".into(),
                    )));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Ok(Ok((mut out, _))) = stdout_reader.join() {
                    out.fill(0);
                }
                let _ = stderr_reader.join();
                return Err(Pkcs12RunError::Fatal(ProxyError::Tls(format!(
                    "等待 PKCS#12 解析进程失败：{err}"
                ))));
            }
        }
    };

    let (mut stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| Pkcs12RunError::Fatal(ProxyError::Tls("读取 PKCS#12 输出线程异常".into())))?
        .map_err(|err| Pkcs12RunError::Fatal(ProxyError::tls(err)))?;
    let (stderr, _) = stderr_reader
        .join()
        .map_err(|_| Pkcs12RunError::Fatal(ProxyError::Tls("读取 PKCS#12 诊断线程异常".into())))?
        .map_err(|err| Pkcs12RunError::Fatal(ProxyError::tls(err)))?;

    if stdout_overflow {
        stdout.fill(0);
        return Err(Pkcs12RunError::Fatal(ProxyError::Tls(
            "PKCS#12 解包后的 PEM 数据超过安全上限".into(),
        )));
    }
    if !status.success() {
        stdout.fill(0);
        return Err(Pkcs12RunError::Failed {
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        });
    }
    Ok(stdout)
}

#[cfg(windows)]
fn unpack_pkcs12_windows_native(
    pkcs12_bytes: &[u8],
    password: &str,
) -> Result<Vec<u8>, ProxyError> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::ffi::c_void;

    if pkcs12_bytes.is_empty() {
        return Err(ProxyError::Tls(
            "PKCS#12 解析失败：文件格式无效或加密算法不受支持".into(),
        ));
    }

    #[repr(C)]
    struct CryptDataBlob {
        cb_data: u32,
        pb_data: *const u8,
    }
    #[repr(C)]
    struct CertContext {
        dw_cert_encoding_type: u32,
        pb_cert_encoded: *const u8,
        cb_cert_encoded: u32,
        p_cert_info: *mut c_void,
        h_cert_store: *mut c_void,
    }
    #[repr(C)]
    struct NCryptBuffer {
        cb_buffer: u32,
        buffer_type: u32,
        pv_buffer: *mut c_void,
    }
    #[repr(C)]
    struct NCryptBufferDesc {
        ul_version: u32,
        c_buffers: u32,
        p_buffers: *mut NCryptBuffer,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn PFXImportCertStore(
            p_pfx: *const CryptDataBlob,
            sz_password: *const u16,
            dw_flags: u32,
        ) -> *mut c_void;
        fn CertEnumCertificatesInStore(
            h_cert_store: *mut c_void,
            p_prev_cert_context: *const CertContext,
        ) -> *const CertContext;
        fn CertCloseStore(h_cert_store: *mut c_void, dw_flags: u32) -> i32;
        fn CryptAcquireCertificatePrivateKey(
            p_cert: *const CertContext,
            dw_flags: u32,
            pv_parameters: *const c_void,
            ph_crypt_prov_or_ncrypt_key: *mut usize,
            pdw_key_spec: *mut u32,
            pf_caller_free_prov_or_ncrypt_key: *mut i32,
        ) -> i32;
    }
    #[link(name = "ncrypt")]
    unsafe extern "system" {
        fn NCryptExportKey(
            h_key: usize,
            h_export_key: usize,
            psz_blob_type: *const u16,
            p_parameter_list: *const NCryptBufferDesc,
            pb_output: *mut u8,
            cb_input: u32,
            pcb_result: *mut u32,
            dw_flags: u32,
        ) -> i32;
        fn NCryptDeleteKey(h_key: usize, dw_flags: u32) -> i32;
    }
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptOpenAlgorithmProvider(
            ph_algorithm: *mut *mut c_void,
            psz_alg_id: *const u16,
            psz_implementation: *const u16,
            dw_flags: u32,
        ) -> i32;
        fn BCryptSetProperty(
            h_object: *mut c_void,
            psz_property: *const u16,
            pb_input: *const u8,
            cb_input: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptGenerateSymmetricKey(
            h_algorithm: *mut c_void,
            ph_key: *mut *mut c_void,
            pb_key_object: *mut u8,
            cb_key_object: u32,
            pb_secret: *const u8,
            cb_secret: u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptDecrypt(
            h_key: *mut c_void,
            pb_input: *const u8,
            cb_input: u32,
            p_padding_info: *const c_void,
            pb_iv: *mut u8,
            cb_iv: u32,
            pb_output: *mut u8,
            cb_output: u32,
            pcb_result: *mut u32,
            dw_flags: u32,
        ) -> i32;
        fn BCryptDestroyKey(h_key: *mut c_void) -> i32;
        fn BCryptCloseAlgorithmProvider(h_algorithm: *mut c_void, dw_flags: u32) -> i32;
    }

    // CRYPT_EXPORTABLE (0x1) | PKCS12_PREFER_CNG_KSP (0x100) | PKCS12_NO_PERSIST_KEY (0x8000)
    const IMPORT_FLAGS: u32 = 0x0000_8101;
    const CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG: u32 = 0x0004_0000;
    let mut pw_utf16: Vec<u16> = password.encode_utf16().chain(std::iter::once(0)).collect();
    let blob = CryptDataBlob {
        cb_data: u32::try_from(pkcs12_bytes.len())
            .map_err(|_| ProxyError::Tls("PKCS#12 归档超过长度上限".into()))?,
        pb_data: pkcs12_bytes.as_ptr(),
    };

    let mut store = unsafe { PFXImportCertStore(&blob, pw_utf16.as_ptr(), IMPORT_FLAGS) };
    let mut last_err = io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
    if store.is_null() && password.is_empty() && last_err == 86 {
        store = unsafe { PFXImportCertStore(&blob, std::ptr::null(), IMPORT_FLAGS) };
        last_err = io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
    }
    pw_utf16.fill(0);

    if store.is_null() {
        // 86 = ERROR_INVALID_PASSWORD, 0x80090005 = NTE_BAD_DATA, 0x80092002 = CRYPT_E_BAD_ENCODE
        if last_err == 86 || last_err == 0x8009_0005 || last_err == 0x8009_0007 {
            return Err(ProxyError::Tls(
                "PKCS#12 解密失败：口令错误或数据完整性校验未通过".into(),
            ));
        }
        return Err(ProxyError::Tls(
            "PKCS#12 解析失败：文件格式无效或加密算法不受支持".into(),
        ));
    }

    struct StoreGuard(*mut c_void);
    impl Drop for StoreGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = CertCloseStore(self.0, 0);
                }
            }
        }
    }
    let _store_guard = StoreGuard(store);

    let mut pem_out = String::new();
    let mut exported_key = false;
    let mut export_err: Option<ProxyError> = None;
    let mut prev: *const CertContext = std::ptr::null();
    loop {
        let cert = unsafe { CertEnumCertificatesInStore(store, prev) };
        if cert.is_null() {
            break;
        }
        prev = cert;
        let cert_ref = unsafe { &*cert };
        if !cert_ref.pb_cert_encoded.is_null() && cert_ref.cb_cert_encoded > 0 {
            let der = unsafe {
                std::slice::from_raw_parts(
                    cert_ref.pb_cert_encoded,
                    cert_ref.cb_cert_encoded as usize,
                )
            };
            crate::tls::append_pem_block_b64(&mut pem_out, "CERTIFICATE", &STANDARD.encode(der));
        }

        let mut h_key: usize = 0;
        let mut key_spec: u32 = 0;
        let mut caller_free: i32 = 0;
        let acquired = unsafe {
            CryptAcquireCertificatePrivateKey(
                cert,
                CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG,
                std::ptr::null(),
                &mut h_key,
                &mut key_spec,
                &mut caller_free,
            )
        };
        if acquired != 0 && h_key != 0 {
            if exported_key || export_err.is_some() {
                if caller_free != 0 {
                    unsafe {
                        let _ = NCryptDeleteKey(h_key, 0);
                    }
                }
                continue;
            }
            let export_res = (|| -> Result<ZeroizeBytes, ProxyError> {
                // Ephemeral local wrap using pbeWithSHAAnd3-KeyTripleDES-CBC (1.2.840.113549.1.12.1.3),
                // iterations=1, salt=1122334455667788, secret="m\0".
                let mut oid = *b"1.2.840.113549.1.12.1.3\0";
                let mut secret: [u16; 2] = [u16::from(b'm'), 0];
                let mut pbe: [u8; 16] = [
                    1, 0, 0, 0, 8, 0, 0, 0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
                ];
                let mut bufs = [
                    NCryptBuffer {
                        cb_buffer: (secret.len() * 2) as u32,
                        buffer_type: 46, // NCRYPTBUFFER_PKCS_SECRET
                        pv_buffer: secret.as_mut_ptr().cast(),
                    },
                    NCryptBuffer {
                        cb_buffer: oid.len() as u32,
                        buffer_type: 41, // NCRYPTBUFFER_PKCS_ALG_OID
                        pv_buffer: oid.as_mut_ptr().cast(),
                    },
                    NCryptBuffer {
                        cb_buffer: pbe.len() as u32,
                        buffer_type: 42, // NCRYPTBUFFER_PKCS_ALG_PARAM
                        pv_buffer: pbe.as_mut_ptr().cast(),
                    },
                ];
                let desc = NCryptBufferDesc {
                    ul_version: 0,
                    c_buffers: bufs.len() as u32,
                    p_buffers: bufs.as_mut_ptr(),
                };
                let blob_type: Vec<u16> = "PKCS8_PRIVATEKEY\0".encode_utf16().collect();
                let mut cb: u32 = 0;
                let st = unsafe {
                    NCryptExportKey(
                        h_key,
                        0,
                        blob_type.as_ptr(),
                        &desc,
                        std::ptr::null_mut(),
                        0,
                        &mut cb,
                        0,
                    )
                };
                if st != 0 || cb == 0 {
                    return Err(ProxyError::Tls(
                        "无法从 Windows CNG 导出 PKCS#8 私钥".into(),
                    ));
                }
                let mut enc = ZeroizeBytes(vec![0u8; cb as usize]);
                let st = unsafe {
                    NCryptExportKey(
                        h_key,
                        0,
                        blob_type.as_ptr(),
                        &desc,
                        enc.0.as_mut_ptr(),
                        cb,
                        &mut cb,
                        0,
                    )
                };
                if st != 0 {
                    return Err(ProxyError::Tls(
                        "读取 Windows CNG PKCS#8 私钥数据失败".into(),
                    ));
                }
                enc.0.truncate(cb as usize);
                let cipher = extract_encrypted_private_key_ciphertext(&enc.0)?;

                // RFC 7292 PKCS#12 SHA-1 KDF key/IV for secret="m\0", salt=1122334455667788, iter=1
                const WRAP_KEY: [u8; 24] = [
                    0x4e, 0xe9, 0x3b, 0x7f, 0xec, 0x98, 0xa6, 0xb2, 0x22, 0xdc, 0x2d, 0xf0, 0x6e,
                    0xbd, 0xa6, 0x32, 0x8f, 0xda, 0x10, 0x77, 0x5e, 0x82, 0x41, 0x12,
                ];
                let mut iv: [u8; 8] = [0x52, 0x21, 0x25, 0x10, 0x0f, 0xa3, 0x46, 0x84];
                let alg_name: Vec<u16> = "3DES\0".encode_utf16().collect();
                let prop_name: Vec<u16> = "ChainingMode\0".encode_utf16().collect();
                let mode_cbc: Vec<u16> = "ChainingModeCBC\0".encode_utf16().collect();

                let mut h_alg: *mut c_void = std::ptr::null_mut();
                if unsafe {
                    BCryptOpenAlgorithmProvider(&mut h_alg, alg_name.as_ptr(), std::ptr::null(), 0)
                } != 0
                {
                    return Err(ProxyError::Tls("无法初始化 Windows BCrypt 3DES".into()));
                }
                let mode_bytes: &[u8] = unsafe {
                    std::slice::from_raw_parts(mode_cbc.as_ptr().cast(), mode_cbc.len() * 2)
                };
                if unsafe {
                    BCryptSetProperty(
                        h_alg,
                        prop_name.as_ptr(),
                        mode_bytes.as_ptr(),
                        mode_bytes.len() as u32,
                        0,
                    )
                } != 0
                {
                    unsafe {
                        let _ = BCryptCloseAlgorithmProvider(h_alg, 0);
                    }
                    return Err(ProxyError::Tls("无法配置 Windows BCrypt 3DES CBC 模式".into()));
                }
                let mut h_sym: *mut c_void = std::ptr::null_mut();
                let gen_st = unsafe {
                    BCryptGenerateSymmetricKey(
                        h_alg,
                        &mut h_sym,
                        std::ptr::null_mut(),
                        0,
                        WRAP_KEY.as_ptr(),
                        WRAP_KEY.len() as u32,
                        0,
                    )
                };
                if gen_st != 0 {
                    unsafe {
                        let _ = BCryptCloseAlgorithmProvider(h_alg, 0);
                    }
                    return Err(ProxyError::Tls("无法创建 Windows BCrypt 解密密钥".into()));
                }
                let mut plain = ZeroizeBytes(vec![0u8; cipher.len()]);
                let mut plain_len: u32 = 0;
                let dec_st = unsafe {
                    BCryptDecrypt(
                        h_sym,
                        cipher.as_ptr(),
                        cipher.len() as u32,
                        std::ptr::null(),
                        iv.as_mut_ptr(),
                        iv.len() as u32,
                        plain.0.as_mut_ptr(),
                        plain.0.len() as u32,
                        &mut plain_len,
                        0x0000_0001, // BCRYPT_BLOCK_PADDING
                    )
                };
                unsafe {
                    let _ = BCryptDestroyKey(h_sym);
                    let _ = BCryptCloseAlgorithmProvider(h_alg, 0);
                }
                if dec_st != 0 {
                    return Err(ProxyError::Tls("解密导出的 PKCS#8 私钥失败".into()));
                }
                plain.0.truncate(plain_len as usize);
                Ok(plain)
            })();

            if caller_free != 0 {
                unsafe {
                    let _ = NCryptDeleteKey(h_key, 0);
                }
            }
            match export_res {
                Ok(pkcs8) => {
                    let mut b64 = ZeroizeBytes(STANDARD.encode(&pkcs8.0).into_bytes());
                    let b64_str = std::str::from_utf8(&b64.0).unwrap_or_default();
                    crate::tls::append_pem_block_b64(&mut pem_out, "PRIVATE KEY", b64_str);
                    b64.0.fill(0);
                    exported_key = true;
                }
                Err(err) => {
                    export_err = Some(err);
                }
            }
        }
    }

    if let Some(err) = export_err {
        return Err(err);
    }

    Ok(pem_out.into_bytes())
}

#[cfg(windows)]
fn read_der_tlv(input: &[u8]) -> Result<(u8, &[u8], &[u8]), ProxyError> {
    if input.len() < 2 {
        return Err(ProxyError::Tls("ASN.1 结构截断".into()));
    }
    let tag = input[0];
    let first_len = input[1];
    let (len, hdr_len) = if first_len & 0x80 == 0 {
        (first_len as usize, 2usize)
    } else {
        let num_bytes = (first_len & 0x7f) as usize;
        if num_bytes == 0 || num_bytes > 4 || input.len() < 2 + num_bytes {
            return Err(ProxyError::Tls("ASN.1 长度字段无效".into()));
        }
        let mut len = 0usize;
        for &b in &input[2..2 + num_bytes] {
            len = (len << 8) | (b as usize);
        }
        (len, 2 + num_bytes)
    };
    if input.len() < hdr_len + len {
        return Err(ProxyError::Tls("ASN.1 内容越界".into()));
    }
    Ok((tag, &input[hdr_len..hdr_len + len], &input[hdr_len + len..]))
}

#[cfg(windows)]
fn extract_encrypted_private_key_ciphertext(der: &[u8]) -> Result<&[u8], ProxyError> {
    let (outer_tag, seq_body, _) = read_der_tlv(der)?;
    if outer_tag != 0x30 {
        return Err(ProxyError::Tls(
            "EncryptedPrivateKeyInfo 外层不是 SEQUENCE".into(),
        ));
    }
    let (alg_tag, _, rest) = read_der_tlv(seq_body)?;
    if alg_tag != 0x30 {
        return Err(ProxyError::Tls("AlgorithmIdentifier 不是 SEQUENCE".into()));
    }
    let (oct_tag, cipher, _) = read_der_tlv(rest)?;
    if oct_tag != 0x04 || cipher.is_empty() {
        return Err(ProxyError::Tls(
            "encryptedData 不是非空 OCTET STRING".into(),
        ));
    }
    Ok(cipher)
}

fn read_bounded(reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut buf = Vec::new();
    reader.take(cap).read_to_end(&mut buf)?;
    let overflow = buf.len() > limit;
    if overflow {
        buf.truncate(limit);
    }
    Ok((buf, overflow))
}
