use super::listener::send_oast_interaction;
use super::payload::now_unix_ms;
use super::*;

struct ParsedDnsQuestion {
    id: u16,
    opcode: u8,
    recursion_desired: bool,
    qtype: u16,
    qclass: u16,
    labels: Vec<String>,
}

fn parse_dns_question(packet: &[u8]) -> Option<ParsedDnsQuestion> {
    if packet.len() < DNS_HEADER_LEN {
        return None;
    }
    if packet[2] & 0x80 != 0 {
        return None;
    }
    let question_count = u16::from_be_bytes([packet[4], packet[5]]);
    if question_count == 0 {
        return None;
    }

    let mut cursor = DNS_HEADER_LEN;
    let mut labels = Vec::new();
    let mut name_len = 0_usize;
    let mut jumps = 0_usize;
    let mut question_end = None;
    let mut bound = packet.len();
    while cursor < bound {
        let length_byte = *packet.get(cursor)?;
        if length_byte == 0 {
            if question_end.is_none() {
                question_end = Some(cursor + 1);
            }
            break;
        }

        if length_byte & 0xC0 == 0xC0 {
            let second_byte = *packet.get(cursor + 1)?;
            let pointer =
                usize::from((u16::from(length_byte & 0x3F) << 8) | u16::from(second_byte));
            if pointer < DNS_HEADER_LEN || pointer >= cursor || pointer >= bound {
                return None;
            }
            if question_end.is_none() {
                question_end = Some(cursor + 2);
            }
            jumps += 1;
            if jumps > DNS_MAX_POINTER_JUMPS {
                return None;
            }
            bound = cursor;
            cursor = pointer;
            continue;
        }

        if length_byte & 0xC0 != 0 {
            return None;
        }
        let label_len = usize::from(length_byte);
        let label_start = cursor.checked_add(1)?;
        let label_end = label_start.checked_add(label_len)?;
        if label_end > bound {
            return None;
        }
        let label = packet.get(label_start..label_end)?;
        let sep = usize::from(!labels.is_empty());
        name_len = name_len.checked_add(label_len)?.checked_add(sep)?;
        if name_len > DNS_NAME_MAX {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).into_owned());
        cursor = label_end;
    }

    let question_end = question_end?;
    let type_end = question_end.checked_add(4)?;
    let type_bytes = packet.get(question_end..type_end)?;
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    Some(ParsedDnsQuestion {
        id: u16::from_be_bytes([packet[0], packet[1]]),
        opcode: u8::try_from((flags >> 11) & 0x0f).unwrap_or_default(),
        recursion_desired: flags & 0x0100 != 0,
        qtype: u16::from_be_bytes([type_bytes[0], type_bytes[1]]),
        qclass: u16::from_be_bytes([type_bytes[2], type_bytes[3]]),
        labels,
    })
}

fn qtype_name(qtype: u16) -> String {
    match qtype {
        1 => "A".to_owned(),
        2 => "NS".to_owned(),
        5 => "CNAME".to_owned(),
        6 => "SOA".to_owned(),
        12 => "PTR".to_owned(),
        15 => "MX".to_owned(),
        16 => "TXT".to_owned(),
        28 => "AAAA".to_owned(),
        33 => "SRV".to_owned(),
        255 => "ANY".to_owned(),
        qtype => format!("TYPE{qtype}"),
    }
}

fn encode_dns_name(labels: &[String]) -> Vec<u8> {
    let mut name = Vec::new();
    for label in labels {
        let label_bytes = label.as_bytes();
        let end = label_bytes.len().min(DNS_LABEL_MAX);
        let Ok(length) = u8::try_from(end) else {
            continue;
        };
        if length == 0 {
            continue;
        }
        name.push(length);
        name.extend_from_slice(&label_bytes[..end]);
    }
    name.push(0);
    name
}

fn encode_dns_formerr(packet: &[u8]) -> Vec<u8> {
    let id = if packet.len() >= 2 {
        u16::from_be_bytes([packet[0], packet[1]])
    } else {
        0
    };
    let request_flags = if packet.len() >= 4 {
        u16::from_be_bytes([packet[2], packet[3]])
    } else {
        0
    };
    let flags = 0x8000_u16
        | ((request_flags >> 11) & 0x000f) << 11
        | (request_flags & 0x0100)
        | u16::from(DNS_RCODE_FORMERR);

    let mut response = Vec::with_capacity(DNS_HEADER_LEN);
    response.extend_from_slice(&id.to_be_bytes());
    response.extend_from_slice(&flags.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response
}

fn encode_dns_response(
    question: &ParsedDnsQuestion,
    matched_token: bool,
    dns_answer_ip: Ipv4Addr,
) -> Vec<u8> {
    let should_answer = matched_token
        && matches!(question.qtype, DNS_QTYPE_A | DNS_QTYPE_ANY)
        && matches!(question.qclass, DNS_QCLASS_IN | DNS_QCLASS_ANY);
    let answer_count = u16::from(should_answer);
    let rcode = if matched_token {
        DNS_RCODE_NOERROR
    } else {
        DNS_RCODE_NXDOMAIN
    };
    let flags = 0x8000_u16
        | (u16::from(question.opcode & 0x0f) << 11)
        | 0x0400
        | if question.recursion_desired {
            0x0100
        } else {
            0
        }
        | u16::from(rcode);

    let mut response =
        Vec::with_capacity(DNS_HEADER_LEN + question.labels.len() * DNS_LABEL_MAX + 32);
    response.extend_from_slice(&question.id.to_be_bytes());
    response.extend_from_slice(&flags.to_be_bytes());
    response.extend_from_slice(&1_u16.to_be_bytes());
    response.extend_from_slice(&answer_count.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response.extend_from_slice(&0_u16.to_be_bytes());
    response.extend_from_slice(&encode_dns_name(&question.labels));
    response.extend_from_slice(&question.qtype.to_be_bytes());
    response.extend_from_slice(&question.qclass.to_be_bytes());

    if should_answer {
        response.extend_from_slice(&[0xC0, 0x0C]);
        response.extend_from_slice(&DNS_QTYPE_A.to_be_bytes());
        response.extend_from_slice(&DNS_QCLASS_IN.to_be_bytes());
        response.extend_from_slice(&60_u32.to_be_bytes());
        response.extend_from_slice(&4_u16.to_be_bytes());
        response.extend_from_slice(&dns_answer_ip.octets());
    }
    response
}

pub(super) async fn run_dns_server(
    socket: UdpSocket,
    events_tx: EventSender,
    cancel: CancellationToken,
    issued_tokens: IssuedTokens,
    dns_answer_ip: Ipv4Addr,
) {
    let mut buffer = vec![0_u8; MAX_RAW_REQUEST_BYTES];
    loop {
        let received = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            received = socket.recv_from(&mut buffer) => received,
        };
        let Ok((length, peer_addr)) = received else {
            if cancel.is_cancelled() {
                break;
            }
            continue;
        };
        if length == 0 {
            continue;
        }

        let packet = buffer[..length].to_vec();
        let parsed = parse_dns_question(&packet);
        let labels = parsed
            .as_ref()
            .map_or_else(Vec::new, |question| question.labels.clone());
        let (token, response) = {
            let book = issued_tokens.read().unwrap_or_else(PoisonError::into_inner);
            let token = book.match_candidates(&labels);
            let response = parsed.as_ref().map_or_else(
                || encode_dns_formerr(&packet),
                |question| encode_dns_response(question, token.is_some(), dns_answer_ip),
            );
            (token, response)
        };

        let deadline = Instant::now() + CONNECTION_TIMEOUT;
        let send_response = socket.send_to(&response, peer_addr);
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = sleep_until(deadline) => {}
            _ = send_response => {}
        }

        let interaction = OastInteraction {
            id: Uuid::new_v4(),
            token,
            protocol: OastProtocol::Dns,
            peer_addr: peer_addr.to_string(),
            summary: parsed.as_ref().map_or_else(
                || "DNS malformed request".to_owned(),
                |question| {
                    let name = if question.labels.is_empty() {
                        ".".to_owned()
                    } else {
                        question.labels.join(".")
                    };
                    format!("{} {}", qtype_name(question.qtype), name)
                },
            ),
            raw_request: packet,
            observed_unix_ms: now_unix_ms(),
        };
        send_oast_interaction(&events_tx, &cancel, deadline, interaction).await;
    }
}
