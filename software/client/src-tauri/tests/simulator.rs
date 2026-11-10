//! End-to-end tests: the link layer talking to the simulator through the real wire protocol.
//!
//! These exercise exactly what the UI does (same `Link::request` calls, same reply shapes) and
//! what the firmware will see (the simulator decodes the same COBS frames).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use attinav_client::link::{Link, NullSink};
use attinav_client::ota;
use attinav_client::sim::{SimControl, open_simulator};
use attinav_core::protocol::{
    DeviceMessage, HostMessage, ShotRequest, Status, StreamRequest, shot_flags, stream_sensors,
};

const T: Duration = Duration::from_secs(5);

fn connect(ctl: SimControl) -> (Link, Arc<Mutex<SimControl>>) {
    let ctl = Arc::new(Mutex::new(ctl));
    let transport = open_simulator(Arc::clone(&ctl));
    (Link::start(Arc::new(NullSink), transport, "sim".into(), true), ctl)
}

/// Fast shots: two samples, no QUIET lead (a shot then takes ~30 ms of simulated time).
async fn fast_config(link: &Link) {
    let mut c = match link.request_one(&HostMessage::GetConfig, T).await.unwrap() {
        DeviceMessage::Config(c) => c,
        other => panic!("{other:?}"),
    };
    c.mag_samples = 2;
    c.quiet_lead_ms = 0;
    link.ack(&HostMessage::SetConfig(c), T).await.unwrap();
}

#[tokio::test]
async fn ping_info_and_config_roundtrip() {
    let (link, _) = connect(SimControl::default());
    assert_eq!(link.ack(&HostMessage::Ping, T).await.unwrap(), 0);
    let info = match link.request_one(&HostMessage::GetInfo, T).await.unwrap() {
        DeviceMessage::Info(i) => i,
        other => panic!("{other:?}"),
    };
    assert_eq!(info.protocol, attinav_core::PROTOCOL_VERSION);
    assert_eq!(info.sensors_ok, 0x3F);
    assert!(!info.time_set);
    link.ack(&HostMessage::SetTime(1_800_000_000), T).await.unwrap();
    let info = match link.request_one(&HostMessage::GetInfo, T).await.unwrap() {
        DeviceMessage::Info(i) => i,
        other => panic!("{other:?}"),
    };
    assert!(info.time_set);
    assert!(info.unix_time >= 1_800_000_000);

    // Invalid configuration is refused with InvalidArgument.
    let bad = attinav_core::protocol::DeviceConfig {
        mag_cycle_count: 5000,
        ..Default::default()
    };
    let err = link.ack(&HostMessage::SetConfig(bad), T).await.unwrap_err();
    assert!(
        matches!(
            err,
            attinav_client::error::ClientError::Nack {
                status: Status::InvalidArgument,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn shot_with_raw_returns_six_frames_and_matches_pose() {
    let (link, _) = connect(SimControl {
        mag_noise_counts: 0.5,
        acc_noise_counts: 0.2,
        azimuth_deg: 200.0,
        inclination_deg: 25.0,
        roll_deg: 10.0,
        ..SimControl::default()
    });
    fast_config(&link).await;
    let replies = link
        .request(
            &HostMessage::Shot(ShotRequest {
                flags: shot_flags::WANT_RAW,
                mag_samples: 0,
                cycle_count: 0,
            }),
            T,
        )
        .await
        .unwrap();
    assert_eq!(replies.len(), 6, "{replies:?}");
    let DeviceMessage::ShotResult(r) = replies[0] else {
        panic!("{:?}", replies[0])
    };
    assert_eq!(
        replies
            .iter()
            .filter(|m| matches!(m, DeviceMessage::ShotRaw(_)))
            .count(),
        5
    );
    // Uncalibrated: hard-iron offsets of a few hundred counts cost several degrees, never tens.
    assert!((r.azimuth_deg - 200.0).abs() < 15.0, "az {}", r.azimuth_deg);
    assert!((r.inclination_deg - 25.0).abs() < 15.0, "inc {}", r.inclination_deg);
    assert!(
        r.flags.has(attinav_core::quality::QualityFlags::NO_CALIBRATION),
        "NO_CALIBRATION expected"
    );
}

#[tokio::test]
async fn busy_abort_and_stream() {
    let (link, _) = connect(SimControl::default());
    // A long shot (default 100 samples at CC 400 ≈ 1.4 s).
    let shot = link.request(
        &HostMessage::Shot(ShotRequest {
            flags: 0,
            mag_samples: 400,
            cycle_count: 400,
        }),
        Duration::from_secs(30),
    );
    let link2 = link.clone();
    let side = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Anything but Ping/Abort is Busy while a shot runs.
        let err = link2.ack(&HostMessage::GetConfig, T).await.unwrap_err();
        assert!(
            matches!(
                err,
                attinav_client::error::ClientError::Nack {
                    status: Status::Busy,
                    ..
                }
            ),
            "{err}"
        );
        link2.ack(&HostMessage::Ping, T).await.unwrap();
        link2.ack(&HostMessage::Abort, T).await.unwrap();
    };
    let (shot, ()) = tokio::join!(shot, side);
    let shot = shot.unwrap();
    assert!(
        matches!(shot[0], DeviceMessage::Ack(a) if a.status == Status::Aborted),
        "{shot:?}"
    );

    // Streaming delivers unsolicited samples (seq 0, not collected by any request).
    let rate = link
        .ack(
            &HostMessage::Stream(StreamRequest {
                enable: true,
                rate_hz: 50,
                sensors: stream_sensors::ALL,
            }),
            T,
        )
        .await
        .unwrap();
    assert_eq!(rate, 50);
    tokio::time::sleep(Duration::from_millis(300)).await;
    link.ack(&HostMessage::Abort, T).await.unwrap();
}

#[tokio::test]
async fn full_calibration_through_the_protocol() {
    let (link, _) = connect(SimControl {
        mag_noise_counts: 2.0,
        acc_noise_counts: 1.0,
        ..SimControl::default()
    });
    fast_config(&link).await;
    link.ack(&HostMessage::CalBegin, T).await.unwrap();
    for group in 0..14u8 {
        for roll in 0..4u8 {
            let rep = match link
                .request_one(&HostMessage::CalShot { group, roll }, T)
                .await
                .unwrap()
            {
                DeviceMessage::CalShotDone(r) => r,
                other => panic!("{other:?}"),
            };
            assert_eq!((rep.shot.group, rep.shot.roll), (group, roll));
        }
    }
    // Retaking replaces, removing works, buffer dump matches.
    link.request_one(&HostMessage::CalShot { group: 3, roll: 1 }, T)
        .await
        .unwrap();
    let left = link
        .ack(&HostMessage::CalRemove { group: 13, roll: 3 }, T)
        .await
        .unwrap();
    assert_eq!(left, 55);
    let dump = link.request(&HostMessage::CalGetShots, T).await.unwrap();
    assert_eq!(dump.len(), 56, "55 shots + Ack");
    assert!(matches!(dump.last(), Some(DeviceMessage::Ack(a)) if a.detail == 55));

    let solved = match link
        .request_one(&HostMessage::CalSolve, Duration::from_secs(60))
        .await
        .unwrap()
    {
        DeviceMessage::CalSolved(s) => s,
        other => panic!("{other:?}"),
    };
    assert!(solved.error.is_none(), "{solved:?}");
    assert!(solved.report.residual_deg < 0.1, "{}", solved.report.residual_deg);
    let idx = link.ack(&HostMessage::CalStore { id: 4242 }, T).await.unwrap();
    assert_eq!(idx, 0);
    let cal = match link.request_one(&HostMessage::CalGet, T).await.unwrap() {
        DeviceMessage::CalData(c) => c,
        other => panic!("{other:?}"),
    };
    assert_eq!(cal.id, 4242);
    assert_eq!(cal.n_sets, 1);

    // A calibrated shot now reproduces the pose closely and is clean.
    let replies = link
        .request(
            &HostMessage::Shot(ShotRequest {
                flags: 0,
                mag_samples: 0,
                cycle_count: 0,
            }),
            T,
        )
        .await
        .unwrap();
    let DeviceMessage::ShotResult(r) = replies[0] else {
        panic!("{:?}", replies[0])
    };
    assert!((r.azimuth_deg - 123.4).abs() < 0.3, "az {}", r.azimuth_deg);
    assert!((r.inclination_deg + 12.3).abs() < 0.3, "inc {}", r.inclination_deg);
    assert!(
        !r.flags.has(attinav_core::quality::QualityFlags::NO_CALIBRATION),
        "calibration must be in use"
    );

    // Round trip of the calibration through CalSet.
    link.ack(&HostMessage::CalClear, T).await.unwrap();
    link.ack(&HostMessage::CalSet(cal), T).await.unwrap();
    let back = match link.request_one(&HostMessage::CalGet, T).await.unwrap() {
        DeviceMessage::CalData(c) => c,
        other => panic!("{other:?}"),
    };
    assert_eq!(back, cal);
}

#[tokio::test]
async fn firmware_update_flow() {
    let (link, _) = connect(SimControl::default());
    let image: Vec<u8> = (0..70_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let stages = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&stages);
    ota::update(&link, &image, [0, 2, 0], move |p| s2.lock().unwrap().push(p.stage))
        .await
        .unwrap();
    let stages = stages.lock().unwrap().clone();
    assert_eq!(stages.first(), Some(&"preflight"));
    assert_eq!(stages.last(), Some(&"done"));
    assert!(stages.iter().filter(|s| **s == "transfer").count() >= 68);

    // The simulated device is silent while "resetting", then comes back with the new version.
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let info = match link.request_one(&HostMessage::GetInfo, T).await.unwrap() {
        DeviceMessage::Info(i) => i,
        other => panic!("{other:?}"),
    };
    assert_eq!(info.firmware, [0, 2, 0]);
    assert_eq!(info.boot_state, 0);

    // A corrupted transfer is rejected at FwFinish.
    link.ack(
        &HostMessage::FwBegin(attinav_core::protocol::FwBegin {
            size: 32,
            crc32: 0xDEAD_BEEF,
            version: [9, 9, 9],
        }),
        T,
    )
    .await
    .unwrap();
    let chunk = attinav_core::protocol::FwChunk::new(0, &[0u8; 32]).unwrap();
    link.ack(&HostMessage::FwChunk(chunk), T).await.unwrap();
    let err = link.ack(&HostMessage::FwFinish, T).await.unwrap_err();
    assert!(
        matches!(
            err,
            attinav_client::error::ClientError::Nack {
                status: Status::BadImage,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn unknown_kind_and_timeouts() {
    let (link, _) = connect(SimControl::default());
    // Requests time out cleanly when the device is silent (simulate by resetting: 300 ms silence).
    link.ack(&HostMessage::Reset, T).await.unwrap();
    let err = link
        .ack(&HostMessage::Ping, Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(matches!(err, attinav_client::error::ClientError::Timeout), "{err}");
    tokio::time::sleep(Duration::from_millis(400)).await;
    link.ack(&HostMessage::Ping, T).await.unwrap();
    link.close();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!link.is_open());
    let err = link.ack(&HostMessage::Ping, T).await.unwrap_err();
    assert!(matches!(err, attinav_client::error::ClientError::NotConnected), "{err}");
}
