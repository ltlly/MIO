use super::*;

pub(in crate::ext_archive) fn proxy_http_229(
    source: &mut Container,
    address: u64,
    byte_budget: u64,
    cancel: &CancellationToken,
) -> io::Result<Decoded<ProxyHttp229>> {
    let mut decoder = decoder_229(source, byte_budget, cancel)?;
    let decoded = decoder.mapped(
        address,
        (1, 0),
        "Proxy HTTP history item",
        |reader, object| {
            Ok(ProxyHttp229 {
                time: reader.primitive(object, 11, |reader| {
                    reader.read(8, |source| source.field_i64(object, 11))
                }),
                request: reader.bytes(object, 15),
                edited_request: reader.bytes(object, 17),
                response: reader.bytes(object, 18),
                edited_response: reader.bytes(object, 20),
                parsed_request: reader.field(object, 2, |reader, address| {
                    reader.mapped(address, (0, 0), "Proxy parsed request", |reader, object| {
                        Ok(ProxyRequest229 {
                            service: reader.field(object, 0, Decoder::service_229),
                        })
                    })
                }),
                unverified_variants: [16, 19]
                    .map(|id| reader.field(object, id, |_, address| Ok(address))),
                highlight: reader.primitive(object, 13, |reader| {
                    reader.read(1, |source| source.field_u8(object, 13))
                }),
                annotations: http_annotations::annotations(reader, object),
            })
        },
    )?;
    decoder.check_cancel()?;
    Ok(decoded)
}

/// Only for root.f2.f1 tabs; f4 stays distinct from every selected history request.
pub(in crate::ext_archive) fn repeater_current_229(
    source: &mut Container,
    address: u64,
    byte_budget: u64,
    cancel: &CancellationToken,
) -> io::Result<Decoded<RepeaterCurrent229>> {
    let mut decoder = decoder_229(source, byte_budget, cancel)?;
    let decoded = decoder.mapped(address, (0, 2), "Repeater current tab", |reader, object| {
        Ok(RepeaterCurrent229 {
            current_request: reader.bytes(object, 4),
            history: reader.field(object, 2, |reader, address| {
                reader.mapped(address, (0, 0), "Repeater history list", Decoder::direct)
            }),
            selected_history: reader.primitive(object, 1, |reader| {
                reader.read(4, |source| source.field_i32(object, 1))
            }),
            title: reader.field(object, 0, |reader, address| {
                reader.mapped(
                    address,
                    (0, 0),
                    "Repeater title",
                    Decoder::chunked_title_229,
                )
            }),
            service: reader.field(object, 3, Decoder::service_229),
            group: repeater_groups::group_reference(reader, object),
        })
    })?;
    decoder.check_cancel()?;
    Ok(decoded)
}
