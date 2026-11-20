use crate::{
    InspectorItem, InspectorMutation, InspectorSection, InspectorSelectionCodec,
    apply_inspector_mutation, decode_inspector_selection, encode_inspector_selection,
    inspect_http_messages,
};

#[test]
fn inspector_edits_json_object_and_array_leaf_paths_and_updates_content_length() {
    let body = br#"{"user":{"name":"alice","age":30,"code":"42"},"items":[{"id":1},"tag"],"meta":{"a.b":true}}"#;
    let req = [
        format!(
            "POST /v1/profile HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes(),
        body.to_vec(),
    ]
    .concat();

    let inspected = inspect_http_messages(&req, None);
    assert_eq!(
        inspected.body_params,
        vec![
            InspectorItem {
                name: "user.name".into(),
                value: "alice".into(),
            },
            InspectorItem {
                name: "user.age".into(),
                value: "30".into(),
            },
            InspectorItem {
                name: "user.code".into(),
                value: "\"42\"".into(),
            },
            InspectorItem {
                name: "items[0].id".into(),
                value: "1".into(),
            },
            InspectorItem {
                name: "items[1]".into(),
                value: "tag".into(),
            },
            InspectorItem {
                name: "meta[\"a.b\"]".into(),
                value: "true".into(),
            },
        ]
    );

    let mut raw = apply_inspector_mutation(
        &req,
        InspectorMutation::Update {
            section: InspectorSection::BodyParams,
            index: 0,
            name: "user.name".into(),
            value: "bob".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Update {
            section: InspectorSection::BodyParams,
            index: 3,
            name: "items[0].id".into(),
            value: "99".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Add {
            section: InspectorSection::BodyParams,
            name: "items[2]".into(),
            value: "extra".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Remove {
            section: InspectorSection::BodyParams,
            index: 1,
        },
    )
    .unwrap();

    let after = inspect_http_messages(&raw, None);
    assert_eq!(
        after.body_params,
        vec![
            InspectorItem {
                name: "user.name".into(),
                value: "bob".into(),
            },
            InspectorItem {
                name: "user.code".into(),
                value: "\"42\"".into(),
            },
            InspectorItem {
                name: "items[0].id".into(),
                value: "99".into(),
            },
            InspectorItem {
                name: "items[1]".into(),
                value: "tag".into(),
            },
            InspectorItem {
                name: "items[2]".into(),
                value: "extra".into(),
            },
            InspectorItem {
                name: "meta[\"a.b\"]".into(),
                value: "true".into(),
            },
        ]
    );
    let raw_str = std::str::from_utf8(&raw).unwrap();
    let (_, next_body) = raw_str.split_once("\r\n\r\n").unwrap();
    let cl = after
        .request_headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .unwrap();
    assert_eq!(cl.value, next_body.len().to_string());
}

#[test]
fn inspector_mutates_multipart_params_preserving_exact_boundary_and_binary_parts() {
    let boundary = "----MioBoundary9f8e7d6c";
    let binary_png = b"\x89PNG\r\n\x1a\n\x00\xff\xfeRAW_BINARY_PAYLOAD";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"username\"\r\n\r\nalice\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n",
    );
    body.extend_from_slice(binary_png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let mut req = format!(
        "POST /upload HTTP/1.1\r\nHost: example.com\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(&body);

    let inspected = inspect_http_messages(&req, None);
    assert_eq!(inspected.body_params.len(), 2);
    assert_eq!(inspected.body_params[0].name, "username");
    assert_eq!(inspected.body_params[0].value, "alice");
    assert_eq!(inspected.body_params[1].name, "avatar");

    let mut raw = apply_inspector_mutation(
        &req,
        InspectorMutation::Update {
            section: InspectorSection::BodyParams,
            index: 0,
            name: "username".into(),
            value: "bob".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Add {
            section: InspectorSection::BodyParams,
            name: "role".into(),
            value: "admin".into(),
        },
    )
    .unwrap();

    let lossy_binary = inspected.body_params[1].value.clone();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Update {
            section: InspectorSection::BodyParams,
            index: 1,
            name: "photo".into(),
            value: lossy_binary,
        },
    )
    .unwrap();
    assert!(
        raw.windows(binary_png.len()).any(|w| w == binary_png),
        "untouched binary upload bytes must remain byte-for-byte identical"
    );
    assert!(
        apply_inspector_mutation(
            &raw,
            InspectorMutation::Add {
                section: InspectorSection::BodyParams,
                name: "bad".into(),
                value: format!("--{boundary}"),
            },
        )
        .is_err()
    );

    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Remove {
            section: InspectorSection::BodyParams,
            index: 0,
        },
    )
    .unwrap();
    let after = inspect_http_messages(&raw, None);
    assert_eq!(after.body_params.len(), 2);
    assert_eq!(after.body_params[0].name, "photo");
    assert_eq!(after.body_params[1].name, "role");
    assert_eq!(after.body_params[1].value, "admin");
    assert!(raw.windows(binary_png.len()).any(|w| w == binary_png));
    let sep = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let cl = after
        .request_headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .unwrap();
    assert_eq!(cl.value, (raw.len() - (sep + 4)).to_string());
}

#[test]
fn inspector_selection_auto_decodes_and_reencodes_url_and_base64_chains() {
    let chained = "eyJ1c2VyIjoiYWxpY2UiLCJyb2xlIjoidXNlciJ9%3D";
    let url_b64 = "eyJ1c2VyIjoiYWxpY2UifQ%3D%3D";
    let decoded = decode_inspector_selection(url_b64);
    assert_eq!(decoded.decoded, r#"{"user":"alice"}"#);
    assert_eq!(
        decoded.steps,
        vec![
            InspectorSelectionCodec::Url {
                plus_for_space: false
            },
            InspectorSelectionCodec::Base64 {
                url_safe: false,
                padded: true
            },
        ]
    );
    assert_eq!(decoded.chain_label(), "URL → Base64");
    let reencoded = encode_inspector_selection(r#"{"user":"bob"}"#, &decoded.steps).unwrap();
    assert_eq!(reencoded, "eyJ1c2VyIjoiYm9iIn0%3D");
    assert_eq!(
        decode_inspector_selection(&reencoded).decoded,
        r#"{"user":"bob"}"#
    );

    let double_url = decode_inspector_selection("%25E4%25B8%25AD%2Btest");
    assert_eq!(double_url.decoded, "中 test");
    assert_eq!(double_url.steps.len(), 2);

    assert!(decode_inspector_selection("admin").steps.is_empty());
    assert!(decode_inspector_selection("12345678").steps.is_empty());
    let _ = chained;
}
