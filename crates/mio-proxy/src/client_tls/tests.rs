use super::*;
use rama::tls::client::TlsClientConfig;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

struct TestPki {
    ca_pem: String,
    leaf_pem: String,
    leaf_der: Vec<u8>,
    leaf_key_pem: String,
    other_key_pem: String,
}

fn generate_test_pki(common_name: &str) -> TestPki {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "MIO Test Client CA");
    ca_params.distinguished_name = ca_dn;
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let ca_pem = ca_cert.pem();
    let issuer = Issuer::new(ca_params, ca_key);

    let mut leaf_params = CertificateParams::new(vec!["client.example.com".into()]).unwrap();
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let mut leaf_dn = DistinguishedName::new();
    leaf_dn.push(DnType::CommonName, common_name);
    leaf_params.distinguished_name = leaf_dn;
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();

    let other_key = KeyPair::generate().unwrap();
    TestPki {
        ca_pem,
        leaf_pem: leaf_cert.pem(),
        leaf_der: leaf_cert.der().to_vec(),
        leaf_key_pem: leaf_key.serialize_pem(),
        other_key_pem: other_key.serialize_pem(),
    }
}

fn export_pkcs12(certs_pem: &str, key_pem: &str, password: &str) -> Vec<u8> {
    let cert_file = PrivateTempFile::create("mio-test-cert", certs_pem.as_bytes()).unwrap();
    let key_file = PrivateTempFile::create("mio-test-key", key_pem.as_bytes()).unwrap();
    let out_file = PrivateTempFile::create("mio-test-p12-out", b"").unwrap();
    let spawn_result = Command::new(resolve_openssl_bin())
        .arg("pkcs12")
        .arg("-export")
        .arg("-inkey")
        .arg(&key_file.path)
        .arg("-in")
        .arg(&cert_file.path)
        .arg("-passout")
        .arg("stdin")
        .arg("-out")
        .arg(&out_file.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawn_result {
        Ok(child) => child,
        #[cfg(windows)]
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return export_pkcs12_windows_dotnet(
                &cert_file.path,
                &key_file.path,
                &out_file.path,
                password,
            );
        }
        Err(err) => panic!("failed to spawn openssl: {err}"),
    };
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(format!("{password}\n").as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "openssl pkcs12 -export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read(&out_file.path).unwrap()
}

#[cfg(windows)]
fn export_pkcs12_windows_dotnet(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    out_path: &std::path::Path,
    password: &str,
) -> Vec<u8> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$certsPem = [System.IO.File]::ReadAllText($env:MIO_TEST_CERT)
$keyPem = [System.IO.File]::ReadAllText($env:MIO_TEST_KEY)
$pwd = [Console]::In.ReadLine()
$certMatches = [regex]::Matches($certsPem, '-----BEGIN CERTIFICATE-----(.*?)-----END CERTIFICATE-----', [System.Text.RegularExpressions.RegexOptions]::Singleline)
$keyB64 = ($keyPem -replace '-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----|-----END [A-Z0-9 ]*PRIVATE KEY-----|\s', '')
$keyDer = [Convert]::FromBase64String($keyB64)
$cng = [System.Security.Cryptography.CngKey]::Import($keyDer, [System.Security.Cryptography.CngKeyBlobFormat]::Pkcs8PrivateBlob)
$prop = [System.Security.Cryptography.CngProperty]::new('Export Policy', [BitConverter]::GetBytes([int]3), [System.Security.Cryptography.CngPropertyOptions]::None)
$cng.SetProperty($prop)
$exportCol = [System.Security.Cryptography.X509Certificates.X509Certificate2Collection]::new()
$matched = $false
for ($i = 0; $i -lt $certMatches.Count; $i++) {
    $der = [Convert]::FromBase64String(($certMatches[$i].Groups[1].Value -replace '\s', ''))
    $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new([byte[]]$der)
    if (-not $matched) {
        try {
            if ($cng.AlgorithmGroup.AlgorithmGroup -eq 'RSA') {
                $rsa = [System.Security.Cryptography.RSACng]::new($cng)
                $cert = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::CopyWithPrivateKey($cert, $rsa)
            } else {
                $ecdsa = [System.Security.Cryptography.ECDsaCng]::new($cng)
                $cert = [System.Security.Cryptography.X509Certificates.ECDsaCertificateExtensions]::CopyWithPrivateKey($cert, $ecdsa)
            }
            $matched = $true
        } catch {}
    }
    [void]$exportCol.Add($cert)
}
if (-not $matched) { throw 'Private key did not match any certificate in bundle' }
$pfxBytes = $exportCol.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $pwd)
[System.IO.File]::WriteAllBytes($env:MIO_TEST_OUT, $pfxBytes)
$cng.Delete()
"#;
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("MIO_TEST_CERT", cert_path)
        .env("MIO_TEST_KEY", key_path)
        .env("MIO_TEST_OUT", out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn powershell.exe for test p12 export");
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(format!("{password}\n").as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "powershell pkcs12 export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read(out_path).unwrap()
}

#[test]
fn pem_parses_split_and_combined_bundles_without_leaking_secrets_in_debug() {
    let pki = generate_test_pki("alice.client.example.com");
    let chain_pem = format!("{}{}", pki.leaf_pem, pki.ca_pem);

    let split_rule = ClientCertificateRule {
        enabled: true,
        host_pattern: "api.example.com".into(),
        format: ClientCertificateFormat::Pem,
        certificate_bytes: chain_pem.as_bytes().to_vec(),
        private_key_pem: pki.leaf_key_pem.as_bytes().to_vec(),
        password: String::new(),
    };
    let rule_debug = format!("{split_rule:?}");
    assert!(!rule_debug.contains("PRIVATE KEY"));

    let parsed_split = split_rule.parse_identity().unwrap();
    assert_eq!(
        parsed_split.summary().subject_hint,
        "alice.client.example.com"
    );
    assert_eq!(parsed_split.summary().cert_count, 2);
    assert_eq!(
        &*parsed_split.certificate_chain()[0],
        pki.leaf_der.as_slice()
    );
    assert!(!format!("{parsed_split:?}").contains("PRIVATE KEY"));

    let cloned = parsed_split.clone();
    assert_eq!(cloned.summary(), parsed_split.summary());
    let _ = cloned.apply_to_tls_config(TlsClientConfig::new());

    let combined_pem = format!("{}{}{}", pki.leaf_pem, pki.ca_pem, pki.leaf_key_pem);
    let combined_rule = ClientCertificateRule {
        enabled: true,
        host_pattern: "*.example.com".into(),
        format: ClientCertificateFormat::Pem,
        certificate_bytes: combined_pem.into_bytes(),
        private_key_pem: Vec::new(),
        password: String::new(),
    };
    let parsed_combined = combined_rule.parse_identity().unwrap();
    assert_eq!(parsed_combined.summary().cert_count, 2);
    assert_eq!(
        parsed_combined.summary().subject_hint,
        "alice.client.example.com"
    );
}

#[test]
fn pem_rejects_key_mismatch_multiple_keys_and_invalid_sizes() {
    let pki = generate_test_pki("mismatch.example.com");

    let mismatch =
        ParsedClientIdentity::from_pem(pki.leaf_pem.as_bytes(), pki.other_key_pem.as_bytes());
    assert!(matches!(mismatch, Err(ProxyError::Tls(_))));

    let reversed_chain = format!("{}{}", pki.ca_pem, pki.leaf_pem);
    assert!(
        ParsedClientIdentity::from_pem(reversed_chain.as_bytes(), pki.leaf_key_pem.as_bytes(),)
            .is_err()
    );

    let two_keys = format!("{}{}", pki.leaf_key_pem, pki.other_key_pem);
    assert!(ParsedClientIdentity::from_pem(pki.leaf_pem.as_bytes(), two_keys.as_bytes()).is_err());

    assert!(ParsedClientIdentity::from_pem(pki.leaf_pem.as_bytes(), b"").is_err());
    assert!(ParsedClientIdentity::from_pem(b"", pki.leaf_key_pem.as_bytes()).is_err());
    assert!(
        ParsedClientIdentity::from_pem(
            &vec![b'a'; MAX_CLIENT_CERT_BYTES + 1],
            pki.leaf_key_pem.as_bytes(),
        )
        .is_err()
    );
}

#[test]
fn pkcs12_parses_password_protected_and_empty_password_archives() {
    let pki = generate_test_pki("p12.client.example.com");
    let ca_then_leaf = format!("{}{}", pki.ca_pem, pki.leaf_pem);
    let p12 = export_pkcs12(&ca_then_leaf, &pki.leaf_key_pem, "p@ssw0rd-secret!");

    let rule = ClientCertificateRule {
        enabled: true,
        host_pattern: "[::1]".into(),
        format: ClientCertificateFormat::Pkcs12,
        certificate_bytes: p12,
        private_key_pem: Vec::new(),
        password: "p@ssw0rd-secret!".into(),
    };
    assert!(!format!("{rule:?}").contains("p@ssw0rd-secret!"));

    let parsed = rule.parse_identity().unwrap();
    assert_eq!(parsed.summary().subject_hint, "p12.client.example.com");
    assert_eq!(parsed.summary().cert_count, 2);
    assert_eq!(&*parsed.certificate_chain()[0], pki.leaf_der.as_slice());

    let empty_pw_p12 = export_pkcs12(&pki.leaf_pem, &pki.leaf_key_pem, "");
    let parsed_empty = ParsedClientIdentity::from_pkcs12(&empty_pw_p12, "").unwrap();
    assert_eq!(parsed_empty.summary().cert_count, 1);
}

#[test]
fn pkcs12_rejects_wrong_password_corrupted_bytes_and_invalid_password_input() {
    let pki = generate_test_pki("locked.client.example.com");
    let p12 = export_pkcs12(&pki.leaf_pem, &pki.leaf_key_pem, "correct-horse");

    let wrong = ParsedClientIdentity::from_pkcs12(&p12, "wrong-battery").unwrap_err();
    let message = wrong.to_string();
    assert!(matches!(wrong, ProxyError::Tls(_)));
    assert!(!message.contains("wrong-battery") && !message.contains("correct-horse"));

    assert!(ParsedClientIdentity::from_pkcs12(b"corrupted-pkcs12", "any").is_err());
    assert!(ParsedClientIdentity::from_pkcs12(&p12, "bad\nline").is_err());
    assert!(
        ParsedClientIdentity::from_pkcs12(&p12, &"x".repeat(MAX_CLIENT_CERT_PASSWORD_BYTES + 1))
            .is_err()
    );
}

#[test]
fn host_matcher_and_ordered_selection_handle_wildcards_case_and_ipv6() {
    assert_eq!(
        ClientCertificateRule::validate_host_pattern("*.Example.COM").unwrap(),
        "*.example.com"
    );
    assert_eq!(
        ClientCertificateRule::validate_host_pattern("[2001:0db8::1]").unwrap(),
        "2001:db8::1"
    );
    assert_eq!(
        ClientCertificateRule::validate_host_pattern("*").unwrap(),
        "*"
    );
    assert_eq!(
        ClientCertificateRule::validate_host_pattern("10.0.*").unwrap(),
        "10.0.*"
    );
    assert_eq!(
        ClientCertificateRule::validate_host_pattern("api.*.example.invalid").unwrap(),
        "api.*.example.invalid"
    );
    assert!(matches_host("*", "api.example.invalid"));
    assert!(matches_host("*", "192.0.2.10"));
    assert!(matches_host("10.0.*", "10.0.42.7"));
    assert!(!matches_host("10.0.*", "10.1.0.7"));
    assert!(matches_host("api.*.example.invalid", "api.v2.example.invalid"));
    assert!(!matches_host("api.*.example.invalid", "web.v2.example.invalid"));

    for invalid in [
        "",
        " ",
        "*.",
        "*.127.0.0.1",
        "example.com:8443",
        "https://example.invalid",
        "[::1",
        "::1]",
        "[example.com]",
        &format!("{}.com", "a".repeat(250)),
    ] {
        assert!(
            ClientCertificateRule::validate_host_pattern(invalid).is_err(),
            "pattern {invalid:?} must be rejected"
        );
        assert!(!matches_host(invalid, "api.example.com"));
    }

    assert!(matches_host("Api.Example.Com", "api.example.com"));
    assert!(!matches_host("api.example.com", "sub.api.example.com"));
    assert!(matches_host("*.example.com", "api.example.com"));
    assert!(matches_host("*.EXAMPLE.COM", "a.b.example.com"));
    assert!(!matches_host("*.example.com", "example.com"));
    assert!(!matches_host("*.example.com", "badexample.com"));
    assert!(!matches_host("*.example.com", ".example.com"));
    assert!(matches_host("[::1]", "::1"));
    assert!(matches_host("::1", "[0:0::1]"));
    assert!(matches_host("127.0.0.1", "127.0.0.1"));

    let first_pki = generate_test_pki("first-cert");
    let second_pki = generate_test_pki("second-cert");
    let rules = vec![
        ClientCertificateRule {
            enabled: false,
            host_pattern: "api.example.com".into(),
            format: ClientCertificateFormat::Pem,
            certificate_bytes: first_pki.leaf_pem.clone().into_bytes(),
            private_key_pem: first_pki.leaf_key_pem.clone().into_bytes(),
            password: String::new(),
        },
        ClientCertificateRule {
            enabled: true,
            host_pattern: "*.example.com".into(),
            format: ClientCertificateFormat::Pem,
            certificate_bytes: second_pki.leaf_pem.into_bytes(),
            private_key_pem: second_pki.leaf_key_pem.into_bytes(),
            password: String::new(),
        },
        ClientCertificateRule {
            enabled: true,
            host_pattern: "api.example.com".into(),
            format: ClientCertificateFormat::Pem,
            certificate_bytes: first_pki.leaf_pem.into_bytes(),
            private_key_pem: first_pki.leaf_key_pem.into_bytes(),
            password: String::new(),
        },
    ];

    let selected = resolve_client_identity(&rules, "API.Example.com")
        .unwrap()
        .unwrap();
    assert_eq!(selected.summary().subject_hint, "second-cert");

    let compiled: Vec<_> = rules
        .iter()
        .cloned()
        .map(|rule| {
            let id = rule.parse_identity().unwrap();
            (rule, id)
        })
        .collect();
    assert_eq!(
        select_client_identity(&compiled, "sub.api.example.com")
            .unwrap()
            .summary()
            .subject_hint,
        "second-cert"
    );
    assert!(
        resolve_client_identity(&rules, "example.com")
            .unwrap()
            .is_none()
    );
}
