// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::super::{Connection, FixedConnectionIdManager, Output, State, LOCAL_IDLE_TIMEOUT};
use super::{
    assert_error, connect_force_idle, connect_with_rtt, default_client, default_server, handshake,
    maybe_authenticate, send_something, split_datagram, AT_LEAST_PTO, DEFAULT_STREAM_DATA,
};
use crate::events::ConnectionEvent;
use crate::frame::StreamType;
use crate::path::PATH_MTU_V6;
use crate::{ConnectionError, Error, QuicVersion};

use neqo_common::{qdebug, Datagram};
use neqo_crypto::{constants::TLS_CHACHA20_POLY1305_SHA256, AuthenticationStatus};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use test_fixture::{self, assertions, fixture_init, loopback, now};

#[test]
fn full_handshake() {
    qdebug!("---- client: generate CH");
    let mut client = default_client();
    let out = client.process(None, now());
    assert!(out.as_dgram_ref().is_some());
    assert_eq!(out.as_dgram_ref().unwrap().len(), PATH_MTU_V6);
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- server: CH -> SH, EE, CERT, CV, FIN");
    let mut server = default_server();
    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- client: cert verification");
    let out = client.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_none());

    assert!(maybe_authenticate(&mut client));

    qdebug!("---- client: SH..FIN -> FIN");
    let out = client.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());
    assert_eq!(*client.state(), State::Connected);

    qdebug!("---- server: FIN -> ACKS");
    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());
    assert_eq!(*server.state(), State::Confirmed);

    qdebug!("---- client: ACKS -> 0");
    let out = client.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_none());
    qdebug!("Output={:0x?}", out.as_dgram_ref());
    assert_eq!(*client.state(), State::Confirmed);
}

#[test]
fn handshake_failed_authentication() {
    qdebug!("---- client: generate CH");
    let mut client = default_client();
    let out = client.process(None, now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- server: CH -> SH, EE, CERT, CV, FIN");
    let mut server = default_server();
    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- client: cert verification");
    let out = client.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_none());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    let authentication_needed = |e| matches!(e, ConnectionEvent::AuthenticationNeeded);
    assert!(client.events().any(authentication_needed));
    qdebug!("---- client: Alert(certificate_revoked)");
    client.authenticated(AuthenticationStatus::CertRevoked, now());

    qdebug!("---- client: -> Alert(certificate_revoked)");
    let out = client.process(None, now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- server: Alert(certificate_revoked)");
    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());
    assert_error(&client, &ConnectionError::Transport(Error::CryptoAlert(44)));
    assert_error(&server, &ConnectionError::Transport(Error::PeerError(300)));
}

#[test]
fn no_alpn() {
    fixture_init();
    let mut client = Connection::new_client(
        "example.com",
        &["bad-alpn"],
        Rc::new(RefCell::new(FixedConnectionIdManager::new(9))),
        loopback(),
        loopback(),
        QuicVersion::default(),
    )
    .unwrap();
    let mut server = default_server();

    handshake(&mut client, &mut server, now(), Duration::new(0, 0));
    // TODO (mt): errors are immediate, which means that we never send CONNECTION_CLOSE
    // and the client never sees the server's rejection of its handshake.
    //assert_error(&client, ConnectionError::Transport(Error::CryptoAlert(120)));
    assert_error(
        &server,
        &ConnectionError::Transport(Error::CryptoAlert(120)),
    );
}

#[test]
fn dup_server_flight1() {
    qdebug!("---- client: generate CH");
    let mut client = default_client();
    let out = client.process(None, now());
    assert!(out.as_dgram_ref().is_some());
    assert_eq!(out.as_dgram_ref().unwrap().len(), PATH_MTU_V6);
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    qdebug!("---- server: CH -> SH, EE, CERT, CV, FIN");
    let mut server = default_server();
    let out_to_rep = server.process(out.dgram(), now());
    assert!(out_to_rep.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out_to_rep.as_dgram_ref());

    qdebug!("---- client: cert verification");
    let out = client.process(Some(out_to_rep.as_dgram_ref().unwrap().clone()), now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    let out = server.process(out.dgram(), now());
    assert!(out.as_dgram_ref().is_none());

    assert!(maybe_authenticate(&mut client));

    qdebug!("---- client: SH..FIN -> FIN");
    let out = client.process(None, now());
    assert!(out.as_dgram_ref().is_some());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    assert_eq!(2, client.stats().packets_rx);
    assert_eq!(0, client.stats().dups_rx);

    qdebug!("---- Dup, ignored");
    let out = client.process(out_to_rep.dgram(), now());
    assert!(out.as_dgram_ref().is_none());
    qdebug!("Output={:0x?}", out.as_dgram_ref());

    // Four packets total received, 1 of them is a dup and one has been dropped because Initial keys
    // are dropped.
    assert_eq!(4, client.stats().packets_rx);
    assert_eq!(1, client.stats().dups_rx);
    assert_eq!(1, client.stats().dropped_rx);
}

// Test that we split crypto data if they cannot fit into one packet.
// To test this we will use a long server certificate.
#[test]
fn crypto_frame_split() {
    let mut client = default_client();

    let mut server = Connection::new_server(
        test_fixture::LONG_CERT_KEYS,
        test_fixture::DEFAULT_ALPN,
        Rc::new(RefCell::new(FixedConnectionIdManager::new(6))),
        QuicVersion::default(),
    )
    .expect("create a server");

    let client1 = client.process(None, now());
    assert!(client1.as_dgram_ref().is_some());

    // The entire server flight doesn't fit in a single packet because the
    // certificate is large, therefore the server will produce 2 packets.
    let server1 = server.process(client1.dgram(), now());
    assert!(server1.as_dgram_ref().is_some());
    let server2 = server.process(None, now());
    assert!(server2.as_dgram_ref().is_some());

    let client2 = client.process(server1.dgram(), now());
    // This is an ack.
    assert!(client2.as_dgram_ref().is_some());
    // The client might have the certificate now, so we can't guarantee that
    // this will work.
    let auth1 = maybe_authenticate(&mut client);
    assert_eq!(*client.state(), State::Handshaking);

    // let server process the ack for the first packet.
    let server3 = server.process(client2.dgram(), now());
    assert!(server3.as_dgram_ref().is_none());

    // Consume the second packet from the server.
    let client3 = client.process(server2.dgram(), now());

    // Check authentication.
    let auth2 = maybe_authenticate(&mut client);
    assert!(auth1 ^ auth2);
    // Now client has all data to finish handshake.
    assert_eq!(*client.state(), State::Connected);

    let client4 = client.process(server3.dgram(), now());
    // One of these will contain data depending on whether Authentication was completed
    // after the first or second server packet.
    assert!(client3.as_dgram_ref().is_some() ^ client4.as_dgram_ref().is_some());

    let _ = server.process(client3.dgram(), now());
    let _ = server.process(client4.dgram(), now());

    assert_eq!(*client.state(), State::Connected);
    assert_eq!(*server.state(), State::Confirmed);
}

/// Run a single ChaCha20-Poly1305 test and get a PTO.
#[test]
fn chacha20poly1305() {
    let mut server = default_server();
    let mut client = Connection::new_client(
        test_fixture::DEFAULT_SERVER_NAME,
        test_fixture::DEFAULT_ALPN,
        Rc::new(RefCell::new(FixedConnectionIdManager::new(0))),
        loopback(),
        loopback(),
        QuicVersion::default(),
    )
    .expect("create a default client");
    client.set_ciphers(&[TLS_CHACHA20_POLY1305_SHA256]).unwrap();
    connect_force_idle(&mut client, &mut server);
}

/// Test that a server can send 0.5 RTT application data.
#[test]
fn send_05rtt() {
    let mut client = default_client();
    let mut server = default_server();

    let c1 = client.process(None, now()).dgram();
    assert!(c1.is_some());
    let s1 = server.process(c1, now()).dgram().unwrap();

    // The server should accept writes at this point.
    let s2 = send_something(&mut server, now());

    // Complete the handshake at the client.
    client.process_input(s1, now());
    maybe_authenticate(&mut client);
    assert_eq!(*client.state(), State::Connected);

    // The client should receive the 0.5-RTT data now.
    client.process_input(s2, now());
    let mut buf = vec![0; DEFAULT_STREAM_DATA.len() + 1];
    let stream_id = client
        .events()
        .find_map(|e| {
            if let ConnectionEvent::RecvStreamReadable { stream_id } = e {
                Some(stream_id)
            } else {
                None
            }
        })
        .unwrap();
    let (l, ended) = client.stream_recv(stream_id, &mut buf).unwrap();
    assert_eq!(&buf[..l], DEFAULT_STREAM_DATA);
    assert!(ended);
}

/// Test that a client buffers 0.5-RTT data when it arrives early.
#[test]
fn reorder_05rtt() {
    let mut client = default_client();
    let mut server = default_server();

    let c1 = client.process(None, now()).dgram();
    assert!(c1.is_some());
    let s1 = server.process(c1, now()).dgram().unwrap();

    // The server should accept writes at this point.
    let s2 = send_something(&mut server, now());

    // We can't use the standard facility to complete the handshake, so
    // drive it as aggressively as possible.
    client.process_input(s2, now());
    assert_eq!(client.stats().saved_datagrams, 1);

    // After processing the first packet, the client should go back and
    // process the 0.5-RTT packet data, which should make data available.
    client.process_input(s1, now());
    // We can't use `maybe_authenticate` here as that consumes events.
    client.authenticated(AuthenticationStatus::Ok, now());
    assert_eq!(*client.state(), State::Connected);

    let mut buf = vec![0; DEFAULT_STREAM_DATA.len() + 1];
    let stream_id = client
        .events()
        .find_map(|e| {
            if let ConnectionEvent::RecvStreamReadable { stream_id } = e {
                Some(stream_id)
            } else {
                None
            }
        })
        .unwrap();
    let (l, ended) = client.stream_recv(stream_id, &mut buf).unwrap();
    assert_eq!(&buf[..l], DEFAULT_STREAM_DATA);
    assert!(ended);
}

#[test]
fn reorder_05rtt_with_0rtt() {
    const RTT: Duration = Duration::from_millis(100);

    let mut client = default_client();
    let mut server = default_server();
    let mut now = connect_with_rtt(&mut client, &mut server, now(), RTT);

    // Include RTT in sending the ticket or the ticket age reported by the
    // client is wrong, which causes the server to reject 0-RTT.
    now += RTT / 2;
    server.send_ticket(now, &[]).unwrap();
    let ticket = server.process_output(now).dgram().unwrap();
    now += RTT / 2;
    client.process_input(ticket, now);
    let token = client.resumption_token().unwrap();
    let mut client = default_client();
    client.enable_resumption(now, &token[..]).unwrap();
    let mut server = default_server();

    // Send ClientHello and some 0-RTT.
    let c1 = send_something(&mut client, now);
    assertions::assert_coalesced_0rtt(&c1[..]);
    // Drop the 0-RTT from the coalesced datagram, so that the server
    // acknowledges the next 0-RTT packet.
    let (c1, _) = split_datagram(&c1);
    let c2 = send_something(&mut client, now);

    // Handle the first packet and send 0.5-RTT in response.  Drop the response.
    now += RTT / 2;
    let _ = server.process(Some(c1), now).dgram().unwrap();
    // The gap in 0-RTT will result in this 0.5 RTT containing an ACK.
    server.process_input(c2, now);
    let s2 = send_something(&mut server, now);

    // Save the 0.5 RTT.
    now += RTT / 2;
    client.process_input(s2, now);
    assert_eq!(client.stats().saved_datagrams, 1);

    // Now PTO at the client and cause the server to re-send handshake packets.
    now += AT_LEAST_PTO;
    let c3 = client.process(None, now).dgram();

    now += RTT / 2;
    let s3 = server.process(c3, now).dgram().unwrap();
    assertions::assert_no_1rtt(&s3[..]);

    // The client should be able to process the 0.5 RTT now.
    // This should contain an ACK, so we are processing an ACK from the past.
    now += RTT / 2;
    client.process_input(s3, now);
    maybe_authenticate(&mut client);
    let c4 = client.process(None, now).dgram();
    assert_eq!(*client.state(), State::Connected);
    assert_eq!(client.loss_recovery.rtt(), RTT);

    now += RTT / 2;
    server.process_input(c4.unwrap(), now);
    assert_eq!(*server.state(), State::Confirmed);
    assert_eq!(server.loss_recovery.rtt(), RTT);
}

/// Test that a server that coalesces 0.5 RTT with handshake packets
/// doesn't cause the client to drop application data.
#[test]
fn coalesce_05rtt() {
    const RTT: Duration = Duration::from_millis(100);
    let mut client = default_client();
    let mut server = default_server();
    let mut now = now();

    // The first exchange doesn't offer a chance for the server to send.
    // So drop the server flight and wait for the PTO.
    let c1 = client.process(None, now).dgram();
    assert!(c1.is_some());
    now += RTT / 2;
    let s1 = server.process(c1, now).dgram();
    assert!(s1.is_some());

    // Drop the server flight.  Then send some data.
    let stream_id = server.stream_create(StreamType::UniDi).unwrap();
    assert!(server.stream_send(stream_id, DEFAULT_STREAM_DATA).is_ok());
    assert!(server.stream_close_send(stream_id).is_ok());

    // Now after a PTO the client can send another packet.
    // The server should then send its entire flight again,
    // including the application data, which it sends in a 1-RTT packet.
    now += AT_LEAST_PTO;
    let c2 = client.process(None, now).dgram();
    assert!(c2.is_some());
    now += RTT / 2;
    let s2 = server.process(c2, now).dgram();
    assert!(s2.is_some());

    // The client should process the datagram.  It can't process the 1-RTT
    // packet until authentication completes though.  So it saves it.
    now += RTT / 2;
    assert_eq!(client.stats().dropped_rx, 0);
    let _ = client.process(s2, now).dgram();
    // This packet will contain an ACK, but we can ignore it.
    assert_eq!(client.stats().dropped_rx, 0);
    assert_eq!(client.stats().packets_rx, 3);
    assert_eq!(client.stats().saved_datagrams, 1);

    // After (successful) authentication, the packet is processed.
    maybe_authenticate(&mut client);
    let c3 = client.process(None, now).dgram();
    assert!(c3.is_some());
    assert_eq!(client.stats().dropped_rx, 0);
    assert_eq!(client.stats().packets_rx, 4);
    assert_eq!(client.stats().saved_datagrams, 1);

    // Allow the handshake to complete.
    now += RTT / 2;
    let s3 = server.process(c3, now).dgram();
    assert!(s3.is_some());
    assert_eq!(*server.state(), State::Confirmed);
    now += RTT / 2;
    let _ = client.process(s3, now).dgram();
    assert_eq!(*client.state(), State::Confirmed);

    assert_eq!(client.stats().dropped_rx, 0);
}

#[test]
fn reorder_handshake() {
    const RTT: Duration = Duration::from_millis(100);
    let mut client = default_client();
    let mut server = default_server();
    let mut now = now();

    let c1 = client.process(None, now).dgram();
    assert!(c1.is_some());

    now += RTT / 2;
    let s1 = server.process(c1, now).dgram();
    assert!(s1.is_some());

    // Drop the Initial packet from this.
    let (_, s_hs) = split_datagram(&s1.unwrap());
    assert!(s_hs.is_some());

    // Pass just the handshake packet in and the client can't handle it.
    now += RTT / 2;
    let res = client.process(s_hs, now);
    assert_ne!(res.callback(), Duration::new(0, 0));
    assert_eq!(client.stats().saved_datagrams, 1);
    assert_eq!(client.stats().packets_rx, 1);

    // Get the server to try again.
    // Though we currently allow the server to arm its PTO timer, use
    // a second client Initial packet to cause it to send again.
    now += AT_LEAST_PTO;
    let c2 = client.process(None, now).dgram();
    now += RTT / 2;
    let s2 = server.process(c2, now).dgram();
    assert!(s2.is_some());

    let (s_init, s_hs) = split_datagram(&s2.unwrap());
    assert!(s_hs.is_some());

    // Processing the Handshake packet first should save it.
    now += RTT / 2;
    client.process_input(s_hs.unwrap(), now);
    assert_eq!(client.stats().saved_datagrams, 2);
    assert_eq!(client.stats().packets_rx, 2);

    client.process_input(s_init, now);
    // Each saved packet should now be "received" again.
    assert_eq!(client.stats().packets_rx, 5);
    maybe_authenticate(&mut client);
    let c3 = client.process(None, now).dgram();
    assert!(c3.is_some());

    // Note that though packets were saved and processed very late,
    // they don't cause the RTT to change.
    now += RTT / 2;
    let s3 = server.process(c3, now).dgram();
    assert_eq!(*server.state(), State::Confirmed);
    assert_eq!(server.loss_recovery.rtt(), RTT);

    now += RTT / 2;
    client.process_input(s3.unwrap(), now);
    assert_eq!(*client.state(), State::Confirmed);
    assert_eq!(client.loss_recovery.rtt(), RTT);
}

#[test]
fn reorder_1rtt() {
    const RTT: Duration = Duration::from_millis(100);
    const PACKETS: usize = 6; // Many, but not enough to overflow cwnd.
    let mut client = default_client();
    let mut server = default_server();
    let mut now = now();

    let c1 = client.process(None, now).dgram();
    assert!(c1.is_some());

    now += RTT / 2;
    let s1 = server.process(c1, now).dgram();
    assert!(s1.is_some());

    now += RTT / 2;
    client.process_input(s1.unwrap(), now);
    maybe_authenticate(&mut client);
    let c2 = client.process(None, now).dgram();
    assert!(c2.is_some());

    // Now get a bunch of packets from the client.
    // Give them to the server before giving it `c2`.
    for _ in 0..PACKETS {
        let d = send_something(&mut client, now);
        server.process_input(d, now + RTT / 2);
    }
    // The server has now received those packets, and saved them.
    // The two extra received are Initial + the junk we use for padding.
    assert_eq!(server.stats().packets_rx, PACKETS + 2);
    assert_eq!(server.stats().saved_datagrams, PACKETS);
    assert_eq!(server.stats().dropped_rx, 1);

    now += RTT / 2;
    let s2 = server.process(c2, now).dgram();
    // The server has now received those packets, and saved them.
    // The two additional are an Initial ACK and Handshake.
    assert_eq!(server.stats().packets_rx, PACKETS * 2 + 4);
    assert_eq!(server.stats().saved_datagrams, PACKETS);
    assert_eq!(server.stats().dropped_rx, 1);
    assert_eq!(*server.state(), State::Confirmed);
    assert_eq!(server.loss_recovery.rtt(), RTT);

    now += RTT / 2;
    client.process_input(s2.unwrap(), now);
    assert_eq!(client.loss_recovery.rtt(), RTT);

    // All the stream data that was sent should now be available.
    let packets = server
        .events()
        .filter_map(|e| {
            if let ConnectionEvent::RecvStreamReadable { stream_id } = e {
                let mut buf = vec![0; DEFAULT_STREAM_DATA.len() + 1];
                let (recvd, fin) = server.stream_recv(stream_id, &mut buf).unwrap();
                assert_eq!(recvd, DEFAULT_STREAM_DATA.len());
                assert!(fin);
                Some(())
            } else {
                None
            }
        })
        .count();
    assert_eq!(packets, PACKETS);
}

#[test]
fn corrupted_initial() {
    let mut client = default_client();
    let mut server = default_server();
    let d = client.process(None, now()).dgram().unwrap();
    let mut corrupted = Vec::from(&d[..]);
    // Find the last non-zero value and corrupt that.
    let (idx, _) = corrupted
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &v)| v != 0)
        .unwrap();
    corrupted[idx] ^= 0x76;
    let dgram = Datagram::new(d.source(), d.destination(), corrupted);
    server.process_input(dgram, now());
    // The server should have received two packets,
    // the first should be dropped, the second saved.
    assert_eq!(server.stats().packets_rx, 2);
    assert_eq!(server.stats().dropped_rx, 1);
    assert_eq!(server.stats().saved_datagrams, 1);
}

#[test]
// Absent path PTU discovery, max v6 packet size should be PATH_MTU_V6.
fn verify_pkt_honors_mtu() {
    let mut client = default_client();
    let mut server = default_server();
    connect_force_idle(&mut client, &mut server);

    let now = now();

    let res = client.process(None, now);
    assert_eq!(res, Output::Callback(LOCAL_IDLE_TIMEOUT));

    // Try to send a large stream and verify first packet is correctly sized
    assert_eq!(client.stream_create(StreamType::UniDi).unwrap(), 2);
    assert_eq!(client.stream_send(2, &[0xbb; 2000]).unwrap(), 2000);
    let pkt0 = client.process(None, now);
    assert!(matches!(pkt0, Output::Datagram(_)));
    assert_eq!(pkt0.as_dgram_ref().unwrap().len(), PATH_MTU_V6);
}
