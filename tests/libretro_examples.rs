use play96::{Button, Session, key};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

static REAL_CORE_TEST_LOCK: Mutex<()> = Mutex::new(());

const GAMEBOY_CORE_ENV: &str = "PLAY96_GAMEBOY_CORE";
const ZX_SPECTRUM_CORE_ENV: &str = "PLAY96_ZX_SPECTRUM_CORE";
const CPM_CORE_ENV: &str = "PLAY96_CPM_CORE";
const EZ180N_CORE_ENV: &str = "PLAY96_EZ180N_CORE";

fn lock_real_core_tests() -> MutexGuard<'static, ()> {
    REAL_CORE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn core_from_env(variable: &str) -> PathBuf {
    let value = env::var_os(variable).unwrap_or_else(|| {
        panic!("{variable} is not set; point it at a compatible libretro core shared library")
    });
    let path = PathBuf::from(value);
    assert!(
        path.is_file(),
        "{variable} points at `{}`, which is not a file",
        path.display()
    );
    path
}

fn build_example(source: &str, artifact: &str) -> PathBuf {
    build_example_with_args(source, artifact, &["build", source])
}

fn build_example_with_args(source: &str, artifact: &str, arguments: &[&str]) -> PathBuf {
    let root = repository_root();
    let artifact = root.join(artifact);
    if artifact.exists() {
        fs::remove_file(&artifact).unwrap_or_else(|error| {
            panic!(
                "failed to remove stale artifact `{}`: {error}",
                artifact.display()
            )
        });
    }

    let output = Command::new(env!("CARGO_BIN_EXE_ezrac"))
        .current_dir(&root)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch ezrac for `{source}`: {error}"));
    assert!(
        output.status.success(),
        "failed to build `{source}`\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        artifact.is_file(),
        "building `{source}` did not create `{}`\nstdout:\n{}\nstderr:\n{}",
        artifact.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    artifact
}

fn set_fat12_entry(fat: &mut [u8], cluster: u16, value: u16) {
    let offset = usize::from(cluster) + usize::from(cluster / 2);
    if cluster & 1 == 0 {
        fat[offset] = value as u8;
        fat[offset + 1] = (fat[offset + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
    } else {
        fat[offset] = (fat[offset] & 0x0f) | ((value << 4) as u8 & 0xf0);
        fat[offset + 1] = (value >> 4) as u8;
    }
}

fn fat_name(name: &str) -> [u8; 11] {
    let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
    assert!(
        stem.len() <= 8 && extension.len() <= 3,
        "invalid 8.3 name `{name}`"
    );
    assert!(name.is_ascii(), "non-ASCII FAT name `{name}`");
    let mut output = [b' '; 11];
    for (slot, byte) in output[..8].iter_mut().zip(stem.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    for (slot, byte) in output[8..].iter_mut().zip(extension.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    output
}

fn write_cpm_disk(label: &str, program: &[u8], extra_files: &[(&str, &[u8])]) -> PathBuf {
    const SECTOR_SIZE: usize = 512;
    const TOTAL_SECTORS: usize = 1_440;
    const SECTORS_PER_CLUSTER: usize = 2;
    const SECTORS_PER_FAT: usize = 3;
    const ROOT_ENTRIES: usize = 112;
    const ROOT_SECTORS: usize = 7;
    const FAT_START_SECTOR: usize = 1;
    const ROOT_START_SECTOR: usize = FAT_START_SECTOR + 2 * SECTORS_PER_FAT;
    const DATA_START_SECTOR: usize = ROOT_START_SECTOR + ROOT_SECTORS;
    const CLUSTER_SIZE: usize = SECTOR_SIZE * SECTORS_PER_CLUSTER;

    let mut disk = vec![0u8; TOTAL_SECTORS * SECTOR_SIZE];
    disk[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    disk[3..11].copy_from_slice(b"EZRAC   ");
    disk[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    disk[13] = SECTORS_PER_CLUSTER as u8;
    disk[14..16].copy_from_slice(&1u16.to_le_bytes());
    disk[16] = 2;
    disk[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    disk[19..21].copy_from_slice(&(TOTAL_SECTORS as u16).to_le_bytes());
    disk[21] = 0xf9;
    disk[22..24].copy_from_slice(&(SECTORS_PER_FAT as u16).to_le_bytes());
    disk[24..26].copy_from_slice(&9u16.to_le_bytes());
    disk[26..28].copy_from_slice(&2u16.to_le_bytes());
    disk[38] = 0x29;
    disk[39..43].copy_from_slice(&0x455a_5241u32.to_le_bytes());
    disk[43..54].copy_from_slice(b"EZRA CPM   ");
    disk[54..62].copy_from_slice(b"FAT12   ");
    disk[510..512].copy_from_slice(&[0x55, 0xaa]);

    let mut fat = vec![0u8; SECTORS_PER_FAT * SECTOR_SIZE];
    fat[0..3].copy_from_slice(&[0xf9, 0xff, 0xff]);
    let files = std::iter::once(("PROGRAM.COM", program))
        .chain(extra_files.iter().copied())
        .collect::<Vec<_>>();
    assert!(files.len() <= ROOT_ENTRIES);

    let root_start = ROOT_START_SECTOR * SECTOR_SIZE;
    let data_start = DATA_START_SECTOR * SECTOR_SIZE;
    let mut next_cluster = 2u16;
    for (index, (name, bytes)) in files.iter().enumerate() {
        let root = root_start + index * 32;
        disk[root..root + 11].copy_from_slice(&fat_name(name));
        disk[root + 11] = 0x20;
        disk[root + 28..root + 32].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        if bytes.is_empty() {
            continue;
        }

        let cluster_count = bytes.len().div_ceil(CLUSTER_SIZE);
        let first_cluster = next_cluster;
        disk[root + 26..root + 28].copy_from_slice(&first_cluster.to_le_bytes());
        for cluster_index in 0..cluster_count {
            let cluster = first_cluster + cluster_index as u16;
            let value = if cluster_index + 1 == cluster_count {
                0x0fff
            } else {
                cluster + 1
            };
            set_fat12_entry(&mut fat, cluster, value);

            let source_start = cluster_index * CLUSTER_SIZE;
            let source_end = (source_start + CLUSTER_SIZE).min(bytes.len());
            let destination = data_start + usize::from(cluster - 2) * CLUSTER_SIZE;
            disk[destination..destination + source_end - source_start]
                .copy_from_slice(&bytes[source_start..source_end]);
        }
        next_cluster += cluster_count as u16;
    }

    let first_fat = FAT_START_SECTOR * SECTOR_SIZE;
    let second_fat = first_fat + SECTORS_PER_FAT * SECTOR_SIZE;
    disk[first_fat..second_fat].copy_from_slice(&fat);
    disk[second_fat..second_fat + fat.len()].copy_from_slice(&fat);

    let directory = repository_root().join("target/play96-cpm");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{label}.img"));
    fs::write(&path, disk).unwrap();
    fs::write(
        path.with_extension("ep128cfg"),
        "machineDetailedType \"EP128_DISK_ISDOS\"\ncontentfilename \"PROGRAM.COM\"\n",
    )
    .unwrap();
    path
}

fn assert_framebuffer(session: &Session, label: &str) {
    let width = session.framebuffer_width();
    let height = session.framebuffer_height();
    assert!(width > 0 && height > 0, "{label} produced no video frame");
    assert_eq!(
        session.framebuffer().len(),
        width as usize * height as usize,
        "{label} returned an incomplete framebuffer"
    );
}

fn assert_non_uniform_frame(session: &Session, label: &str) {
    assert_framebuffer(session, label);
    let first = session.framebuffer()[0];
    assert!(
        session.framebuffer().iter().any(|pixel| *pixel != first),
        "{label} produced a uniform framebuffer (frame hash {:016x})",
        session.frame_hash()
    );
}

fn capture(session: &Session, name: &str) {
    let directory = repository_root().join("target/play96-captures");
    fs::create_dir_all(&directory).unwrap_or_else(|error| {
        panic!(
            "failed to create capture directory `{}`: {error}",
            directory.display()
        )
    });
    let path = directory.join(format!("{name}.png"));
    session
        .write_png(&path)
        .unwrap_or_else(|error| panic!("failed to write `{}`: {error}", path.display()));
    eprintln!("play96 capture: {}", path.display());
}

fn tap_key(session: &mut Session, keycode: u32) {
    session.set_key(keycode, true);
    session.run_frames(2).unwrap();
    session.set_key(keycode, false);
    session.run_frames(2).unwrap();
}

fn tap_key_chord(session: &mut Session, keycodes: &[u32]) {
    for keycode in keycodes {
        session.set_key(*keycode, true);
    }
    session.run_frames(2).unwrap();
    for keycode in keycodes {
        session.set_key(*keycode, false);
    }
    session.run_frames(2).unwrap();
}

fn start_zx_loaded_code(session: &mut Session) {
    // Fuse fast-loads the CODE block and leaves a clean ZX BASIC prompt. Enter
    // RANDOMIZE with T, switch to extended mode, then enter USR with L.
    tap_key(session, key::T);
    tap_key(session, key::SPACE);
    tap_key_chord(session, &[key::LEFT_SHIFT, key::LEFT_CTRL]);
    tap_key(session, key::L);
    tap_key(session, key::SPACE);
    for digit in "32768".bytes() {
        tap_key(session, u32::from(digit));
    }
    tap_key(session, key::RETURN);
}

fn is_blue(pixel: u32) -> bool {
    let red = (pixel >> 16) & 0xff;
    let green = (pixel >> 8) & 0xff;
    let blue = pixel & 0xff;
    blue > red && blue > green
}

fn pulse_button(session: &mut Session, button: Button, held_frames: usize, settle_frames: usize) {
    session.set_button(button, true);
    session.run_frames(held_frames).unwrap();
    session.set_button(button, false);
    session.run_frames(settle_frames).unwrap();
}

fn heard_audio_during(session: &mut Session, frames: usize) -> bool {
    let mut heard_audio = false;
    for _ in 0..frames {
        session.run_frame().unwrap();
        heard_audio |= session.audio_samples().iter().any(|sample| *sample != 0);
    }
    heard_audio
}

fn assert_deterministic_save_state(session: &mut Session, label: &str) {
    let directory = repository_root().join("target/play96-states");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{label}.state"));

    session.save_state(&path).unwrap_or_else(|error| {
        panic!("{label} core does not provide a usable save state: {error}")
    });
    session.run_frames(5).unwrap();
    let expected_frame = session.frame_hash();
    let expected_audio = session.audio_hash();

    session.load_state(&path).unwrap();
    session.run_frames(5).unwrap();
    assert_eq!(
        session.frame_hash(),
        expected_frame,
        "{label} video diverged after restoring a save state"
    );
    assert_eq!(
        session.audio_hash(),
        expected_audio,
        "{label} audio diverged after restoring a save state"
    );

    let _ = fs::remove_file(path);
}

fn round_trip_save_state(session: &mut Session, label: &str) {
    let directory = repository_root().join("target/play96-states");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{label}.state"));
    session.save_state(&path).unwrap_or_else(|error| {
        panic!("{label} core does not provide a usable save state: {error}")
    });
    session.run_frames(5).unwrap();
    session.load_state(&path).unwrap();
    session.run_frames(5).unwrap();
    let _ = fs::remove_file(path);
}

fn open_session(core: &Path, cartridge: &Path, label: &str) -> Session {
    Session::new(core, cartridge).unwrap_or_else(|error| {
        panic!(
            "failed to load {label} with core `{}` and content `{}`: {error}",
            core.display(),
            cartridge.display()
        )
    })
}

#[test]
#[ignore = "requires PLAY96_GAMEBOY_CORE pointing at a third-party Game Boy libretro core"]
fn gameboy_examples_run_on_real_core() {
    let _guard = lock_real_core_tests();
    let core = core_from_env(GAMEBOY_CORE_ENV);
    let background = build_example(
        "examples/gameboy/background/src/main.ezra",
        "examples/gameboy/background/target/gameboy-dmg-lr35902/src/main.gb",
    );
    let color_input = build_example(
        "examples/gameboy/color-input/src/main.ezra",
        "examples/gameboy/color-input/target/gameboy-color-lr35902/src/main.gbc",
    );
    let input_audio = build_example(
        "examples/gameboy/input-audio/src/main.ezra",
        "examples/gameboy/input-audio/target/gameboy-dmg-lr35902/src/main.gb",
    );
    let serial_hello = build_example(
        "examples/gameboy/serial-hello/src/main.ezra",
        "examples/gameboy/serial-hello/target/gameboy-dmg-lr35902/src/main.gb",
    );
    let sprite = build_example(
        "examples/gameboy/sprite/src/main.ezra",
        "examples/gameboy/sprite/target/gameboy-dmg-lr35902/src/main.gb",
    );

    {
        let mut game = open_session(&core, &background, "Game Boy background example");
        game.run_frames(300).unwrap();
        assert_eq!(
            (game.framebuffer_width(), game.framebuffer_height()),
            (160, 144),
            "Game Boy background example used unexpected video geometry"
        );
        assert_non_uniform_frame(&game, "Game Boy background example");
        assert_deterministic_save_state(&mut game, "gameboy-background");
        capture(&game, "gameboy-background");
    }

    {
        let mut game = open_session(&core, &sprite, "Game Boy sprite example");
        game.run_frames(300).unwrap();
        assert_non_uniform_frame(&game, "Game Boy sprite example");
        let corner = game.pixel_xrgb(0, 0).unwrap();
        assert!(
            game.framebuffer().iter().any(|pixel| *pixel != corner),
            "Game Boy sprite was not visible against its background"
        );
        capture(&game, "gameboy-sprite");
    }

    {
        let mut game = open_session(&core, &color_input, "Game Boy Color input example");
        game.run_frames(300).unwrap();
        assert_non_uniform_frame(&game, "Game Boy Color input example");
        let warm_palette = game.frame_hash();
        let warm_pixel = game.pixel_xrgb(0, 0).unwrap();

        pulse_button(&mut game, Button::A, 2, 3);
        let cool_palette = game.frame_hash();
        let cool_pixel = game.pixel_xrgb(0, 0).unwrap();
        assert_ne!(
            cool_palette, warm_palette,
            "A input did not switch the Game Boy Color background palette"
        );
        assert_ne!(
            cool_pixel, warm_pixel,
            "A input did not change the sampled Game Boy Color palette entry"
        );

        pulse_button(&mut game, Button::Right, 2, 3);
        let scrolled = game.frame_hash();
        assert_ne!(
            scrolled, cool_palette,
            "Right input did not scroll the Game Boy Color background"
        );

        pulse_button(&mut game, Button::Left, 2, 3);
        let unscrolled = game.frame_hash();
        assert_ne!(
            unscrolled, scrolled,
            "Left input did not reverse the Game Boy Color scroll"
        );
        pulse_button(&mut game, Button::B, 2, 3);
        assert_ne!(
            game.pixel_xrgb(0, 0).unwrap(),
            cool_pixel,
            "B input did not restore the warm Game Boy Color palette"
        );
        capture(&game, "gameboy-color-input");
    }

    {
        let mut game = open_session(&core, &input_audio, "Game Boy input/audio example");
        game.run_frames(300).unwrap();
        game.set_button(Button::A, true);
        let audible = heard_audio_during(&mut game, 30);
        game.clear_buttons();
        assert!(audible, "A input did not produce Game Boy audio samples");
        capture(&game, "gameboy-input-audio");
    }

    {
        let mut game = open_session(&core, &serial_hello, "Game Boy serial example");
        game.run_frames(300).unwrap();
        assert_framebuffer(&game, "Game Boy serial example");
        capture(&game, "gameboy-serial-hello");
    }
}

#[test]
#[ignore = "requires PLAY96_ZX_SPECTRUM_CORE pointing at the Fuse libretro core"]
fn zx_spectrum_example_runs_on_real_core() {
    let _guard = lock_real_core_tests();
    let core = core_from_env(ZX_SPECTRUM_CORE_ENV);
    let cartridge = build_example(
        "examples/zxspectrum-z80/hello/src/main.ezra",
        "examples/zxspectrum-z80/hello/target/zxspectrum-z80/src/zx-hello.tap",
    );

    let mut game = open_session(&core, &cartridge, "ZX Spectrum hello example");
    game.run_frames(1_200).unwrap();
    if !is_blue(game.pixel_xrgb(2, 2).unwrap()) {
        start_zx_loaded_code(&mut game);
        game.run_frames(300).unwrap();
    }
    assert_non_uniform_frame(&game, "ZX Spectrum hello example");

    let border = game.pixel_xrgb(2, 2).unwrap();
    capture(&game, "zx-spectrum-hello");
    assert!(
        is_blue(border),
        "ZX Spectrum example did not set a blue border; border pixel was #{border:06x}. The tape may have loaded without starting its code"
    );

    round_trip_save_state(&mut game, "zx-spectrum-hello");
    let restored_border = (0..60).any(|_| {
        game.run_frame().unwrap();
        is_blue(game.pixel_xrgb(2, 2).unwrap())
    });
    assert!(
        restored_border,
        "ZX Spectrum state restore did not redraw the program's blue border within 60 frames"
    );
}

#[test]
#[ignore = "requires PLAY96_CPM_CORE pointing at the ep128emu libretro core"]
fn cpm_examples_run_on_real_core() {
    let _guard = lock_real_core_tests();
    let core = core_from_env(CPM_CORE_ENV);
    let examples = [
        (
            "console-output-source",
            "examples/cpm-z80/console-output.ezra",
            false,
            false,
        ),
        (
            "file-read-source",
            "examples/cpm-z80/file-read.ezra",
            false,
            true,
        ),
        (
            "line-input-source",
            "examples/cpm-z80/line-input.ezra",
            false,
            false,
        ),
        (
            "console-output-assembly",
            "examples/cpm-z80/console-output.asm",
            true,
            false,
        ),
        ("exit-assembly", "examples/cpm-z80/exit.asm", true, false),
        (
            "file-read-assembly",
            "examples/cpm-z80/file-read.asm",
            true,
            true,
        ),
        (
            "line-input-assembly",
            "examples/cpm-z80/line-input.asm",
            true,
            false,
        ),
    ];

    let mut disks = Vec::new();
    for (label, source, assembly, needs_readme) in examples {
        let stem = Path::new(source).file_stem().unwrap().to_str().unwrap();
        let artifact = format!("examples/cpm-z80/target/cpm-2.2-z80/{stem}.com");
        let arguments = if assembly {
            vec![
                "build",
                "--target",
                "cpm-2.2-z80",
                "--input-kind",
                "assembly",
                source,
            ]
        } else {
            vec!["build", "--target", "cpm-2.2-z80", source]
        };
        let program = build_example_with_args(source, &artifact, &arguments);
        let program = fs::read(program).unwrap();
        let extra_files: &[(&str, &[u8])] = if needs_readme {
            &[("README.TXT", b"E from an emulated CP/M disk\r\n")]
        } else {
            &[]
        };
        disks.push((label, write_cpm_disk(label, &program, extra_files)));
    }

    let mut frame_hashes = Vec::new();
    for (label, disk) in disks {
        eprintln!("loading CP/M {label} from {}", disk.display());
        let mut machine = open_session(&core, &disk, &format!("CP/M {label} example"));
        eprintln!("running CP/M {label}");
        machine.run_frames(1_500).unwrap();
        capture(&machine, &format!("cpm-{label}"));
        assert_non_uniform_frame(&machine, &format!("CP/M {label} example"));
        if label == "console-output-source" {
            assert_deterministic_save_state(&mut machine, "cpm-console-output");
        }
        frame_hashes.push((label, machine.frame_hash()));
    }

    let console_hash = frame_hashes
        .iter()
        .find(|(label, _)| *label == "console-output-source")
        .unwrap()
        .1;
    let exit_hash = frame_hashes
        .iter()
        .find(|(label, _)| *label == "exit-assembly")
        .unwrap()
        .1;
    assert_ne!(
        console_hash, exit_hash,
        "CP/M console output matched the no-output exit example; the generated program may not have executed"
    );
}

#[test]
#[ignore = "requires PLAY96_EZ180N_CORE pointing at the ez180N libretro core"]
fn ez180n_examples_run_on_real_core() {
    let _guard = lock_real_core_tests();
    let core = core_from_env(EZ180N_CORE_ENV);
    let hello = build_example(
        "examples/ez180n/hello/src/main.ezra",
        "examples/ez180n/hello/target/ez180n-ez80/src/ez180n-hello.gaem",
    );
    let jumping = build_example(
        "examples/ez180n/jumping/src/main.ezra",
        "examples/ez180n/jumping/target/ez180n-ez80/src/ezra-game.gaem",
    );
    let meteor_runner = build_example(
        "examples/ez180n/meteor-runner/src/main.ezra",
        "examples/ez180n/meteor-runner/target/ez180n-ez80/src/meteor-runner.gaem",
    );

    {
        let mut game = open_session(&core, &hello, "ez180N hello example");
        let audible = heard_audio_during(&mut game, 10);
        assert_eq!(
            (game.framebuffer_width(), game.framebuffer_height()),
            (720, 504),
            "ez180N hello example used unexpected video geometry"
        );
        assert_non_uniform_frame(&game, "ez180N hello example");
        assert!(
            audible,
            "ez180N hello example did not play its startup sound"
        );
        capture(&game, "ez180n-hello");
    }

    {
        let mut game = open_session(&core, &jumping, "ez180N jumping example");
        game.run_frames(10).unwrap();
        assert_non_uniform_frame(&game, "ez180N jumping example");
        let stationary = game.frame_hash();
        pulse_button(&mut game, Button::Right, 6, 2);
        assert_ne!(
            game.frame_hash(),
            stationary,
            "Right input did not move the ez180N jumping player"
        );
        let moved = game.frame_hash();
        game.set_button(Button::A, true);
        let jumped = (0..10).any(|_| {
            game.run_frame().unwrap();
            game.frame_hash() != moved
        });
        game.clear_buttons();
        assert!(
            jumped,
            "A input did not make the ez180N jumping player jump"
        );
        capture(&game, "ez180n-jumping");
    }

    {
        let mut game = open_session(&core, &meteor_runner, "ez180N meteor runner example");
        let audible = heard_audio_during(&mut game, 180);
        assert_non_uniform_frame(&game, "ez180N meteor runner example");
        assert!(audible, "ez180N meteor runner did not produce audio");
        pulse_button(&mut game, Button::Right, 6, 2);
        capture(&game, "ez180n-meteor-runner");
    }
}
