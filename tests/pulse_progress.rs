//! Native progress/cancellation contracts, exercised with either RAW backend.
#![cfg(any(feature = "rawloader", feature = "rawler"))]

#[path = "support/dng_builder.rs"]
mod dng_builder;
use dng_builder::*;
use how_far::{Complete, NoPulse, Outcome, Stop, StopOnly, StopReason, Total};
use how_far_along::{FnPulse, Phase, PulseTree, Status};
use zenraw::{DemosaicMethod, OutputMode, RawDecodeConfig, RawError};

fn values(tag: u16, kind: u16, count: u32, value: Vec<u8>) -> Entry {
    Entry {
        tag,
        kind,
        count,
        value,
    }
}

fn fixture(w: u16, h: u16, rgb: bool, float: bool, xtrans: bool, orientation: u16) -> Vec<u8> {
    let cpp = if rgb { 3 } else { 1 };
    let mut entries = vec![
        short(0x100, w),
        short(0x101, h),
        short(0x102, if float { 32 } else { 16 }),
        short(0x103, 1),
        short(0x106, if rgb { 34892 } else { 32803 }),
        ascii(0x10f, "ZenRaw"),
        ascii(0x110, "PulseFixture"),
        long(0x111, 0),
        short(0x112, orientation),
        short(0x115, cpp),
        long(0x116, h as u32),
        long(
            0x117,
            w as u32 * h as u32 * cpp as u32 * if float { 4 } else { 2 },
        ),
        short(0x11c, 1),
        short(0x153, if float { 3 } else { 1 }),
        bytes(0xc612, &[1, 4, 0, 0]),
        long(0xc61d, if float { 1 } else { 65535 }),
        // A real color matrix and neutral white balance exercise rawler's
        // DngPipeline branch as well as rawloader's basic color pipeline.
        values(
            0xc621,
            10,
            9,
            [1i32, 0, 0, 0, 1, 0, 0, 0, 1]
                .iter()
                .flat_map(|n| [n.to_le_bytes(), 1i32.to_le_bytes()].concat())
                .collect(),
        ),
        values(
            0xc628,
            5,
            3,
            [1u32, 1, 1]
                .iter()
                .flat_map(|n| [n.to_le_bytes(), 1u32.to_le_bytes()].concat())
                .collect(),
        ),
        values(
            0xc61f,
            LONG,
            2,
            [1u32, 1].iter().flat_map(|n| n.to_le_bytes()).collect(),
        ),
        values(
            0xc620,
            LONG,
            2,
            [w as u32 - 2, h as u32 - 2]
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect(),
        ),
    ];
    if !rgb {
        let size = if xtrans { 6u16 } else { 2 };
        entries.push(values(
            0x828d,
            SHORT,
            2,
            [size, size].iter().flat_map(|n| n.to_le_bytes()).collect(),
        ));
        entries.push(bytes(
            0x828e,
            if xtrans {
                &[
                    1, 0, 1, 1, 0, 1, 2, 1, 2, 0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 2, 1, 1, 2, 1, 0, 1,
                    0, 2, 1, 2, 1, 2, 1, 1, 2, 1,
                ]
            } else {
                &[0, 1, 1, 2]
            },
        ));
    }
    let mut strip = Vec::new();
    for i in 0..w as usize * h as usize * cpp as usize {
        let sample = (i * 61 % 65535) as u16;
        if float {
            strip.extend_from_slice(&(sample as f32 / 65535.0).to_le_bytes());
        } else {
            strip.extend_from_slice(&sample.to_le_bytes());
        }
    }
    strip.extend_from_slice(&[0; 64]);
    build_tiff(entries, &strip, 0x111)
}

fn stopped(error: &whereat::At<RawError>) -> bool {
    matches!(error.error(), RawError::Stopped(_))
}

#[test]
fn tracked_decode_counts_exact_work_and_preserves_pixels() {
    for (rgb, float, xtrans) in [
        (false, false, false),
        (false, false, true),
        (true, false, false),
        // rawloader 0.37.2 rejects uncompressed 32-bit float DNG samples.
        #[cfg(feature = "rawler")]
        (true, true, false),
        #[cfg(feature = "rawler")]
        (false, true, false),
    ] {
        for mode in [
            OutputMode::CameraRaw,
            OutputMode::Linear,
            OutputMode::Develop,
        ] {
            for orientation in 1..=8 {
                for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
                    let data = fixture(131, 99, rgb, float, xtrans, orientation);
                    let config = RawDecodeConfig::new()
                        .with_output(mode)
                        .with_demosaic(method)
                        .with_exposure_ev(if orientation % 2 == 0 { 0.0 } else { 0.5 })
                        .with_crop(orientation % 3 != 0);
                    let base = zenraw::decode(&data, &config, None).expect("unobserved decode");
                    let no_pulse = zenraw::decode(&data, &config, Some(&NoPulse)).unwrap();
                    let tree =
                        PulseTree::new(Phase::new("decode", Total::Unknown), how_far::Unstoppable);
                    let observer = tree.observer();
                    let tracked = zenraw::decode(&data, &config, Some(&tree)).unwrap();
                    let snapshot = observer.snapshot();
                    assert_eq!(
                        snapshot.status,
                        Status::Running,
                        "borrowed root must stay open"
                    );
                    assert_eq!(snapshot.children.len(), 7);
                    for child in &snapshot.children {
                        match child.status {
                            Status::Finished(Outcome::Succeeded) => assert_eq!(
                                child.total,
                                Total::Exact(child.completed),
                                "{rgb}/{float}/{xtrans}/{mode:?}/{orientation}/{method:?}: {child:?}"
                            ),
                            Status::Finished(Outcome::Skipped) => assert_eq!(child.completed, 0),
                            _ => panic!("unfinished child: {child:?}"),
                        }
                        assert!(!child.overrun(), "{child:?}");
                    }
                    assert_eq!(
                        base.pixels.copy_to_contiguous_bytes(),
                        tracked.pixels.copy_to_contiguous_bytes()
                    );
                    assert_eq!(
                        base.pixels.copy_to_contiguous_bytes(),
                        no_pulse.pixels.copy_to_contiguous_bytes()
                    );
                    assert_eq!(
                        (base.info.width, base.info.height),
                        (tracked.info.width, tracked.info.height)
                    );
                    tree.complete_classified(Ok::<_, whereat::At<RawError>>(()), stopped)
                        .unwrap();
                    assert_eq!(
                        observer.snapshot().status,
                        Status::Finished(Outcome::Succeeded)
                    );
                }
            }
        }
    }
}

#[test]
fn progress_callbacks_can_cancel_inside_batches_and_after_final_report() {
    for target in [
        "Normalize",
        "Reconstruct RGB",
        "Crop",
        "Orient",
        "Render output",
    ] {
        for reason in [StopReason::Cancelled, StopReason::TimedOut] {
            let data = fixture(1024, 769, false, false, false, 6);
            let tree = FnPulse::new("decode", move |cx| {
                if cx.phase_name() == target {
                    let snapshot = cx.snapshot();
                    let child = snapshot.children.iter().find(|c| c.name == target).unwrap();
                    if child.completed > 0 {
                        return Err(reason);
                    }
                }
                Ok(())
            });
            let error =
                zenraw::decode(&data, &RawDecodeConfig::default(), Some(&tree)).unwrap_err();
            assert!(matches!(error.error(), RawError::Stopped(r) if *r == reason));
            let snapshot = tree.observer().snapshot();
            let index = snapshot
                .children
                .iter()
                .position(|c| c.name == target)
                .unwrap();
            assert_eq!(
                snapshot.children[index].status,
                Status::Finished(Outcome::Cancelled)
            );
            for child in &snapshot.children[index + 1..] {
                assert_eq!(child.status, Status::Finished(Outcome::NotRun));
            }
            tree.complete_classified(Err::<(), _>(error), stopped)
                .unwrap_err();
        }
    }
}

#[test]
fn cancelled_parse_does_not_run_later_stages_and_probe_stays_borrowed() {
    let data = fixture(17, 19, false, false, false, 1);
    let tree = FnPulse::new("decode", |cx| {
        if cx.phase_name() == "Parse RAW" {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(zenraw::decode(&data, &RawDecodeConfig::default(), Some(&tree)).is_err());
    let snapshot = tree.observer().snapshot();
    assert_eq!(snapshot.children[0].completed, 0);
    assert_eq!(
        snapshot.children[0].status,
        Status::Finished(Outcome::Cancelled)
    );
    assert!(
        snapshot.children[1..]
            .iter()
            .all(|c| c.status == Status::Finished(Outcome::NotRun))
    );
    tree.complete_as(Outcome::Cancelled);

    let tree = PulseTree::new(Phase::new("probe", Total::Unknown), how_far::Unstoppable);
    let observer = tree.observer();
    zenraw::probe(&data, Some(&tree)).unwrap();
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.status, Status::Running);
    assert_eq!(snapshot.children[0].total, Total::Exact(1));
    assert_eq!(snapshot.children[0].completed, 1);
    tree.complete_as(Outcome::Succeeded);
}

#[test]
fn a_single_non_sticky_stop_is_never_lost() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Once {
        calls: AtomicUsize,
        at: usize,
    }
    impl Stop for Once {
        fn check(&self) -> Result<(), StopReason> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == self.at {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    for rgb in [false, true] {
        let data = fixture(131, 99, rgb, false, false, 3);
        let config = RawDecodeConfig::default();
        let counter = Once {
            calls: AtomicUsize::new(0),
            at: usize::MAX,
        };
        zenraw::decode(&data, &config, Some(&StopOnly::borrowed(&counter))).unwrap();
        for at in 0..counter.calls.load(Ordering::Relaxed) {
            let once = Once {
                calls: AtomicUsize::new(0),
                at,
            };
            let error =
                zenraw::decode(&data, &config, Some(&StopOnly::borrowed(&once))).unwrap_err();
            assert!(
                matches!(error.error(), RawError::Stopped(StopReason::TimedOut)),
                "{rgb}/{at}: {error}"
            );
        }
    }
}

#[test]
fn probe_checks_after_its_final_report_and_failures_do_not_earn_success() {
    let data = fixture(17, 19, false, false, false, 1);
    let pulse = FnPulse::new("probe", |cx| {
        if cx.snapshot().children.iter().any(|c| c.completed == 1) {
            Err(StopReason::TimedOut)
        } else {
            Ok(())
        }
    });
    let error = zenraw::probe(&data, Some(&pulse)).unwrap_err();
    assert!(matches!(
        error.error(),
        RawError::Stopped(StopReason::TimedOut)
    ));
    assert_eq!(
        pulse.observer().snapshot().children[0].status,
        Status::Finished(Outcome::Cancelled)
    );
    pulse
        .complete_classified(Err::<(), _>(error), stopped)
        .unwrap_err();

    let pulse = PulseTree::new(
        Phase::new("bad input", Total::Unknown),
        how_far::Unstoppable,
    );
    let error =
        zenraw::decode(&[0xff; 128], &RawDecodeConfig::default(), Some(&pulse)).unwrap_err();
    let snapshot = pulse.observer().snapshot();
    assert_eq!(
        snapshot.children[0].status,
        Status::Finished(Outcome::Failed)
    );
    assert!(
        snapshot.children[1..]
            .iter()
            .all(|c| c.status == Status::Finished(Outcome::NotRun))
    );
    pulse
        .complete_classified(Err::<(), _>(error), stopped)
        .unwrap_err();
}
