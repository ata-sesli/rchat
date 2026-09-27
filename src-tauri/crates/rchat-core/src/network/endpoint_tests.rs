use super::endpoint::*;
use futures::StreamExt;
use std::net::SocketAddr;

fn response(id: [u8; 12], endpoint: SocketAddr) -> Vec<u8> {
    let SocketAddr::V4(endpoint) = endpoint else {
        panic!("IPv4 fixture")
    };
    let mut packet = vec![0x01, 0x01, 0, 12, 0x21, 0x12, 0xa4, 0x42];
    packet.extend(id);
    packet.extend([0, 0x20, 0, 8, 0, 1]);
    packet.extend((endpoint.port() ^ 0x2112).to_be_bytes());
    packet.extend((u32::from(*endpoint.ip()) ^ 0x2112a442).to_be_bytes());
    packet
}

#[test]
fn stun_replies_require_matching_transaction_and_valid_bounds() {
    let id = [7; 12];
    let addr = "198.51.100.4:23456".parse().unwrap();
    let packet = response(id, addr);
    assert_eq!(parse_response(&packet, &id), Some(addr));
    assert_eq!(parse_response(&packet, &[8; 12]), None);
    for end in 0..packet.len() {
        assert_eq!(parse_response(&packet[..end], &id), None);
    }
    let mut invalid = packet.clone();
    invalid[4] = 0;
    assert_eq!(parse_response(&invalid, &id), None);
    let mut invalid = packet;
    invalid[23] = 1;
    assert_eq!(parse_response(&invalid, &id), None);
    for private in [
        "127.0.0.1:1234",
        "192.168.1.1:1234",
        "169.254.1.1:1234",
        "0.0.0.0:1234",
    ] {
        assert_eq!(
            parse_response(&response(id, private.parse().unwrap()), &id),
            None
        );
    }
}

#[test]
fn stale_or_invalidated_observations_are_not_fresh() {
    let mut observation = Observation::default();
    let now = std::time::Instant::now();
    let addr = "198.51.100.4:23456".parse().unwrap();
    observation.record(addr, now);
    assert_eq!(observation.fresh(now), Some(addr));
    assert_eq!(observation.fresh(now + MAX_OBSERVATION_AGE), None);
    observation.invalidate();
    assert_eq!(observation.fresh(now), None);
    assert_eq!(observation.last_known, Some(addr));
}

#[test]
fn advertisement_excludes_stale_public_endpoint_and_preserves_local_listeners() {
    let now = std::time::Instant::now();
    let mut observation = Observation::default();
    observation.record("198.51.100.4:12345".parse().unwrap(), now);
    let listeners = vec![
        "/ip4/192.168.1.3/udp/8000/quic-v1".to_string(),
        "/ip4/0.0.0.0/udp/8000/quic-v1".to_string(),
        "/ip4/127.0.0.1/tcp/8000".to_string(),
    ];
    assert_eq!(
        advertised_addresses(&observation, &listeners, now),
        vec![
            "/ip4/198.51.100.4/udp/12345/quic-v1",
            "/ip4/192.168.1.3/udp/8000/quic-v1"
        ]
    );
    assert_eq!(
        advertised_addresses(&observation, &listeners, now + MAX_OBSERVATION_AGE),
        vec!["/ip4/192.168.1.3/udp/8000/quic-v1"]
    );
}

#[tokio::test]
async fn active_quic_socket_refreshes_changed_mapping_and_preserves_connections() {
    let observer = EndpointObserver::default();
    let runtime = observer.runtime();
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_quic_config(|mut config| {
            config.runtime = Some(runtime);
            config
        })
        .with_behaviour(|_| libp2p::swarm::dummy::Behaviour)
        .unwrap()
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();
    swarm
        .listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap())
        .unwrap();
    let listen = loop {
        if let libp2p::swarm::SwarmEvent::NewListenAddr { address, .. } =
            swarm.select_next_some().await
        {
            break address;
        }
    };
    let peer_id = *swarm.local_peer_id();
    let local_port = listen
        .iter()
        .find_map(|p| {
            if let libp2p::multiaddr::Protocol::Udp(port) = p {
                Some(port)
            } else {
                None
            }
        })
        .unwrap();
    let task = tokio::spawn(async move {
        loop {
            swarm.select_next_some().await;
        }
    });
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let server_observer = observer.clone();
    let responder = tokio::spawn(async move {
        for port in [12345, 23456, 34567] {
            let mut buf = [0; 1024];
            let (len, source) = server.recv_from(&mut buf).await.unwrap();
            assert_eq!(len, 20);
            assert_eq!(
                source.port(),
                local_port,
                "STUN must use the actual QUIC socket"
            );
            let attacker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            attacker
                .send_to(
                    &response(
                        buf[8..20].try_into().unwrap(),
                        "198.51.100.9:9999".parse().unwrap(),
                    ),
                    source,
                )
                .await
                .unwrap();
            server
                .send_to(
                    &response([0; 12], "198.51.100.9:9998".parse().unwrap()),
                    source,
                )
                .await
                .unwrap();
            if port == 23456 {
                server_observer.invalidate();
            }
            let endpoint = format!("198.51.100.4:{port}").parse().unwrap();
            server
                .send_to(&response(buf[8..20].try_into().unwrap(), endpoint), source)
                .await
                .unwrap();
        }
    });
    let first = observer.refresh_from(&[server_addr], true).await.unwrap();
    assert_eq!(first.port(), 12345);
    assert_eq!(
        observer.refresh_from(&[server_addr], false).await.unwrap(),
        first
    );
    observer.invalidate();
    assert!(
        observer.refresh_from(&[server_addr], false).await.is_err(),
        "reject a response from before a network change"
    );
    assert_eq!(observer.observation().last_known, Some(first));
    assert!(observer
        .observation()
        .fresh(std::time::Instant::now())
        .is_none());
    let changed = observer.refresh_from(&[server_addr], false).await.unwrap();
    assert_eq!(changed.port(), 34567);
    responder.await.unwrap();

    // Same live listener still accepts an authenticated QUIC connection.
    let mut client = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_quic()
        .with_behaviour(|_| libp2p::swarm::dummy::Behaviour)
        .unwrap()
        .build();
    client
        .dial(listen.with(libp2p::multiaddr::Protocol::P2p(peer_id)))
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(
                client.select_next_some().await,
                libp2p::swarm::SwarmEvent::ConnectionEstablished { .. }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    observer.invalidate();
    assert!(observer.refresh_from(&[server_addr], false).await.is_err());
    let state = observer.observation();
    assert_eq!(state.last_known, Some(changed));
    assert!(state.fresh(std::time::Instant::now()).is_none());
    task.abort();
}
