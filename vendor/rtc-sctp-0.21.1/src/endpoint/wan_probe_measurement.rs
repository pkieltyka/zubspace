//! Diagnostic only: a real pair of SCTP associations over a virtual
//! link. Payload sizes/cadence come from the retained Zubspace WAN capture;
//! this is neither Chrome nor full gameplay/UDP acceptance.
use super::*;

fn impair_new_datagrams<T>(
    queue: &mut VecDeque<T>,
    previous_len: usize,
    random: &mut u32,
    drop_per_mille: u32,
) -> u32 {
    let mut dropped = 0;
    for packet in queue.split_off(previous_len) {
        // Deterministic diagnostic generator, unrelated to security nonces.
        *random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        if u64::from(*random) * 1000 < u64::from(drop_per_mille) * (1u64 << 32) {
            dropped += 1;
        } else {
            queue.push_back(packet);
        }
    }
    dropped
}

#[test]
#[ignore = "SCTP deadline/cwnd measurement, not an acceptance gate"]
fn measure_timed_game_probe_delivery_over_virtual_link() -> Result<()> {
    const GAME: u16 = 7;
    const CONTROL: u16 = 9;
    for ack_mode in [AckMode::NoDelay, AckMode::Normal] {
        for rtt_ms in [0, 180, 240] {
            for warm_bulk in [false, true] {
                for drop_per_mille in [0, 5] {
                    let (mut pair, client, server) = create_association_pair(ack_mode, 0)?;
                    establish_session_pair(&mut pair, client, server, GAME)?;
                    establish_session_pair(&mut pair, client, server, CONTROL)?;
                    // TestEndpoint already delivers the actual SCTP datagrams through
                    // its in-memory link. Avoid its redundant real-socket drain.
                    pair.client.socket = None;
                    pair.server.socket = None;
                    pair.latency = Duration::from_millis(rtt_ms / 2);
                    pair.client_stream(client, GAME)?.set_reliability_params(
                        true,
                        ReliabilityType::Timed,
                        80,
                    )?;
                    pair.server_stream(server, GAME)?.set_reliability_params(
                        true,
                        ReliabilityType::Timed,
                        80,
                    )?;
                    if warm_bulk {
                        let now = pair.time;
                        pair.client_stream(client, CONTROL)?.write_sctp(
                            now,
                            &Bytes::from(vec![0x55; 64 * 1024]),
                            PayloadProtocolIdentifier::Binary,
                        )?;
                        pair.server_stream(server, CONTROL)?.write_sctp(
                            now,
                            &Bytes::from(vec![0x55; 64 * 1024]),
                            PayloadProtocolIdentifier::Binary,
                        )?;
                        pair.drive();
                        for (handle, is_client) in [(client, true), (server, false)] {
                            let mut stream = if is_client {
                                pair.client_stream(handle, CONTROL)?
                            } else {
                                pair.server_stream(handle, CONTROL)?
                            };
                            let mut bytes = vec![0; 64 * 1024];
                            assert_eq!(stream.read_sctp()?.unwrap().read(&mut bytes)?, bytes.len());
                        }
                        pair.drive();
                    }
                    let initial_client_cwnd = pair.client_conn_mut(client).cwnd;
                    let initial_server_cwnd = pair.server_conn_mut(server).cwnd;
                    let start = pair.time;
                    let mut frame = 0u64;
                    let mut probes_sent = 0u32;
                    let mut probe_echoes = std::collections::HashSet::new();
                    let mut movement_received = 0usize;
                    let mut snapshots_received = 0usize;
                    let mut probe_received = 0usize;
                    let (mut c2s_random, mut s2c_random) = (41, 42);
                    let (mut c2s_drops, mut s2c_drops) = (0, 0);
                    let mut peak_client_buffered = 0usize;
                    let mut peak_server_buffered = 0usize;
                    let mut bytes = vec![0u8; 2048];
                    for ms in 0..42_000u64 {
                        pair.time = start + Duration::from_millis(ms);
                        if ms < 40_000 && ms >= frame * 1000 / 60 {
                            let now = pair.time;
                            pair.client_stream(client, GAME)?.write_sctp(
                                now,
                                &Bytes::from(vec![0x03; 635]),
                                PayloadProtocolIdentifier::Binary,
                            )?;
                            pair.server_stream(server, GAME)?.write_sctp(
                                now,
                                &Bytes::from(vec![0x70; 192]),
                                PayloadProtocolIdentifier::Binary,
                            )?;
                            if frame % 5 == 0 {
                                pair.client_stream(client, CONTROL)?.write_sctp(
                                    now,
                                    &Bytes::from_static(&[0x7d; 9]),
                                    PayloadProtocolIdentifier::Binary,
                                )?;
                            }
                            frame += 1;
                        }
                        if ms < 40_000 && ms % 500 == 0 {
                            probes_sent += 1;
                            let mut probe = vec![0x81; 30];
                            probe[1..5].copy_from_slice(&probes_sent.to_le_bytes());
                            let now = pair.time;
                            pair.server_stream(server, GAME)?.write_sctp(
                                now,
                                &Bytes::from(probe),
                                PayloadProtocolIdentifier::Binary,
                            )?;
                        }
                        let pending_server = pair.server.inbound.len();
                        pair.drive_client();
                        c2s_drops += impair_new_datagrams(
                            &mut pair.server.inbound,
                            pending_server,
                            &mut c2s_random,
                            drop_per_mille,
                        );
                        let pending_client = pair.client.inbound.len();
                        pair.drive_server();
                        s2c_drops += impair_new_datagrams(
                            &mut pair.client.inbound,
                            pending_client,
                            &mut s2c_random,
                            drop_per_mille,
                        );
                        while let Some(chunks) = pair.client_stream(client, GAME)?.read_sctp()? {
                            let len = chunks.read(&mut bytes)?;
                            if bytes[0] == 0x81 {
                                assert_eq!(len, 30);
                                probe_received += 1;
                                bytes[0] = 0x82;
                                let now = pair.time;
                                pair.client_stream(client, GAME)?.write_sctp(
                                    now,
                                    &Bytes::copy_from_slice(&bytes[..len]),
                                    PayloadProtocolIdentifier::Binary,
                                )?;
                            } else {
                                assert_eq!(bytes[0], 0x70);
                                snapshots_received += 1;
                            }
                        }
                        while let Some(chunks) = pair.server_stream(server, GAME)?.read_sctp()? {
                            let len = chunks.read(&mut bytes)?;
                            if bytes[0] == 0x82 {
                                assert_eq!(len, 30);
                                let sequence = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
                                assert!(sequence > 0 && sequence <= probes_sent);
                                assert!(
                                    probe_echoes.insert(sequence),
                                    "SCTP must deliver each echo once"
                                );
                            } else {
                                assert_eq!(bytes[0], 0x03);
                                movement_received += 1;
                            }
                        }
                        while let Some(chunks) = pair.server_stream(server, CONTROL)?.read_sctp()? {
                            assert_eq!(chunks.read(&mut bytes)?, 9);
                        }
                        peak_client_buffered = peak_client_buffered
                            .max(pair.client_conn_mut(client).buffered_amount());
                        peak_server_buffered = peak_server_buffered
                            .max(pair.server_conn_mut(server).buffered_amount());
                    }
                    assert_eq!(probes_sent, 80);
                    if rtt_ms == 0 && drop_per_mille == 0 {
                        assert_eq!(probe_echoes.len(), 80);
                        assert_eq!(movement_received, frame as usize);
                        assert_eq!(snapshots_received, frame as usize);
                    }
                    let final_client_cwnd = pair.client_conn_mut(client).cwnd;
                    let final_server_cwnd = pair.server_conn_mut(server).cwnd;
                    println!(
                        "{{\"ackMode\":\"{ack_mode}\",\"rttMs\":{rtt_ms},\"warmBulk\":{warm_bulk},\"linkDropPerMille\":{drop_per_mille},\"c2sDatagramDrops\":{c2s_drops},\"s2cDatagramDrops\":{s2c_drops},\"frames\":{frame},\"movementReceived\":{movement_received},\"snapshotsReceived\":{snapshots_received},\"probesSent\":{probes_sent},\"probesReceived\":{probe_received},\"echoesReceived\":{},\"initialClientCwnd\":{initial_client_cwnd},\"initialServerCwnd\":{initial_server_cwnd},\"finalClientCwnd\":{final_client_cwnd},\"finalServerCwnd\":{final_server_cwnd},\"peakClientBuffered\":{peak_client_buffered},\"peakServerBuffered\":{peak_server_buffered}}}",
                        probe_echoes.len()
                    );
                }
            }
        }
    }
    Ok(())
}
