use super::*;
use super::edit::has_key;
use super::settings::entries;
use crate::disk::DriveKind;
use crate::diskio::{DiskSpeed, NoiseMode};
use crate::joystick::JoystickType;
use crate::lpt_dac::LptDacType;
use crate::mixer::{Channel, ChorusPreset, ReverbPreset, SbFilter};
use crate::video::composite::CompositeSettings;
use crate::video::shader::CrtSettings;

fn scratch(name: &str) -> PathBuf {
    let dir = std::path::absolute(PathBuf::from("target/test_config_unit").join(name)).unwrap();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn vr_settings_saved_before_auto_are_auto() {
    let old = parse("[vr]\nquality=high\nresolution=100\n", Path::new("/cfg"), None);
    assert_eq!((old.vr.quality, old.vr.resolution), (crate::vr::VrQuality::Auto, None));
    let set = parse("[vr]\nquality=low\nresolution=70\n", Path::new("/cfg"), None);
    assert_eq!((set.vr.quality, set.vr.resolution), (crate::vr::VrQuality::Low, Some(70)));
    let new = parse("[vr]\nquality=high\nresolution=100\nmsaa=auto\n", Path::new("/cfg"), None);
    assert_eq!((new.vr.quality, new.vr.resolution), (crate::vr::VrQuality::High, Some(100)));
}

#[test]
fn portable_user_dir() {
    let exe = scratch("portable");
    let profile = Some(PathBuf::from("/profile/rust-dos"));
    assert_eq!(user_dir_for(None, profile.clone()), profile);
    assert_eq!(user_dir_for(Some(&exe), profile.clone()), profile);
    fs::write(exe.join(FILE_NAME), "").unwrap();
    assert_eq!(user_dir_for(Some(&exe), profile), Some(exe.clone()));
    assert_eq!(user_dir_for(Some(&exe), None), Some(exe));
}

#[test]
fn locate_priority() {
    let base = scratch("locate");
    let cwd = base.join("work");
    let exe = base.join("bin");
    let default = base.join("home/rust-dos/rust-dos.conf");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&exe).unwrap();
    fs::create_dir_all(default.parent().unwrap()).unwrap();
    fs::write(base.join("custom.conf"), "").unwrap();

    // Nothing yet: the default path is where the template goes
    assert_eq!(
        locate(None, &cwd, None, Some(default.clone())),
        Ok(Located::NotFound {
            default: Some(default.clone())
        })
    );

    fs::write(&default, "").unwrap();
    assert_eq!(
        locate(None, &cwd, None, Some(default.clone())),
        Ok(Located::Found(default.clone(), ConfigSource::UserDefault))
    );

    // A file next to the executable beats the per-user one
    fs::write(exe.join(FILE_NAME), "").unwrap();
    assert_eq!(
        locate(None, &cwd, Some(&exe), Some(default.clone())),
        Ok(Located::Found(exe.join(FILE_NAME), ConfigSource::ExeDir))
    );

    fs::write(cwd.join(FILE_NAME), "").unwrap();
    assert_eq!(
        locate(None, &cwd, Some(&exe), Some(default.clone())),
        Ok(Located::Found(
            cwd.join(FILE_NAME),
            ConfigSource::WorkingDir
        ))
    );

    // Relative --config paths are taken from the working directory
    assert_eq!(
        locate(
            Some(Path::new("../custom.conf")),
            &cwd,
            None,
            Some(default.clone())
        ),
        Ok(Located::Found(
            cwd.join("../custom.conf"),
            ConfigSource::CommandLine
        ))
    );
    assert!(locate(Some(Path::new("missing.conf")), &cwd, None, Some(default)).is_err());
}

#[test]
fn parses_sections_drives_and_autoexec() {
    let text = "\u{FEFF}# comment\r\n\
        [Emulator]\r\n\
        scale = 3\r\n\
        cycles = 3000\r\n\
        \r\n\
        [DRIVES]\r\n\
        c = ~/dos\r\n\
        A: = floppy floppy -label disk1\r\n\
        D=\"My CD\" cdrom\r\n\
        E=C:\\Games\\Dos -ro\r\n\
        ; another comment\r\n\
        [autoexec]\r\n\
        @echo off\r\n\
        MOUNT F ~/x\r\n\
        # skipped\r\n\
        D:\r\n";
    let base = Path::new("/cfg");
    let home = Path::new("/home/u");
    let config = parse(text, base, Some(home));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.scale, Some(3));
    assert_eq!(config.cycles, Some(CpuSpeed::Fixed(3000)));

    let c = config.drive(2).unwrap();
    assert_eq!(c.path, home.join("dos"));
    assert_eq!(c.opts.kind, DriveKind::HardDisk);
    let a = config.drive(0).unwrap();
    assert_eq!(a.path, base.join("floppy"));
    assert_eq!(a.opts.kind, DriveKind::Floppy);
    assert_eq!(a.opts.label.as_deref(), Some("disk1"));
    let d = config.drive(3).unwrap();
    assert_eq!(d.path, base.join("My CD"));
    assert_eq!(d.opts.kind, DriveKind::CdRom);
    let e = config.drive(4).unwrap();
    assert!(e.path.to_string_lossy().ends_with(r"C:\Games\Dos"));
    assert!(e.opts.read_only);

    assert_eq!(config.autoexec, ["@echo off", "MOUNT F ~/x", "D:"]);
}

#[test]
fn problems_become_warnings() {
    let text = "orphan=1\n\
        [emulator]\n\
        scale=0\n\
        colour=blue\n\
        [drives]\n\
        Z=/tmp\n\
        7=/tmp\n\
        D=/one\n\
        D=/two\n\
        E=/x zip\n\
        nonsense\n\
        [modem]\n\
        ignored=1\n";
    let config = parse(text, Path::new("/cfg"), None);
    let joined = config.warnings.join("\n");
    for expected in [
        "line 1: setting outside of a section",
        "line 3: invalid scale '0'",
        "line 4: unknown setting 'colour'",
        "line 6: drive Z: is reserved",
        "line 7: invalid drive letter '7'",
        "line 9: drive D: defined twice",
        "line 10: drive E: Unknown option 'zip'",
        "line 11: expected key=value",
        "line 12: unknown section [modem]",
    ] {
        assert!(
            joined.contains(expected),
            "missing '{}' in:\n{}",
            expected,
            joined
        );
    }
    assert_eq!(config.warnings.len(), 9, "{}", joined);
    assert_eq!(config.drive(3).unwrap().path, Path::new("/two"));
    assert_eq!(config.scale, None);
}

#[test]
fn sound_section() {
    let config = parse(
        "[sound]\nsbtype=sbpro2\nsbbase=240\nirq=5\ndma=3\nopl=opl2\nsoundfont=gm.sf2\ngus=false\n",
        Path::new("/cfg"),
        None,
    );
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let sb = config.sound.card().unwrap();
    assert_eq!((sb.model, sb.base, sb.irq, sb.dma8), (crate::sb::SbModel::SbPro2, 0x240, 5, 3));
    assert!(!config.sound.opl3);
    assert_eq!(config.sound.soundfont.as_deref(), Some(Path::new("/cfg/gm.sf2")));
    assert_eq!(sb.blaster(), "A240 I5 D3 T4");

    let config = parse("[sound]\nsbtype=none\nirq=4\n", Path::new("/cfg"), None);
    assert_eq!(config.sound.card(), None);
    assert!(config.warnings[0].contains("invalid irq '4'"));
}

#[test]
fn channel_2_can_start_with_its_gate_high() {
    assert!(!parse("[sound]
", Path::new("/cfg"), None).sound.pit2_gate_at_start);
    let config = parse("[sound]
pit2_gate_at_start=true
", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert!(config.sound.pit2_gate_at_start);
    let config = parse("[sound]
pit2_gate_at_start=maybe
", Path::new("/cfg"), None);
    assert!(!config.sound.pit2_gate_at_start);
    assert!(config.warnings[0].contains("pit2_gate_at_start"), "{:?}", config.warnings);
}

#[test]
fn a_dac_on_the_parallel_port() {
    let config = parse("[sound]\nlpt_dac=Disney\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.sound.lpt_dac, LptDacType::Disney);
    let config = parse("[sound]\nlpt_dac=ston1\n", Path::new("/cfg"), None);
    assert_eq!((config.sound.lpt_dac, config.warnings.len()), (LptDacType::None, 1));
}

#[test]
fn ultrasound_settings() {
    let config = parse(
        "[sound]\ngusbase=260\ngusirq=11\ngusdma=6\nultradir=D:\\GUS\nmidisynth=gus\n",
        Path::new("/cfg"),
        None,
    );
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let gus = config.sound.ultrasound().unwrap();
    assert_eq!((gus.base, gus.irq, gus.dma, gus.ultradir().as_str()), (0x260, 11, 6, "D:\\GUS"));
    assert_eq!(gus.ultrasnd(), "260,6,6,11,11");
    assert!(!gus.builtin());
    assert_eq!(config.sound.midisynth, MidiSynth::Gus);

    let config = parse("[sound]\ngus=off\n", Path::new("/cfg"), None);
    assert_eq!(config.sound.ultrasound(), None);

    let config = parse(
        "[sound]\ngusbase=230\ngusirq=4\ngusdma=2\nmidisynth=mt64\ngusdrive=Z\n",
        Path::new("/cfg"),
        None,
    );
    assert_eq!(config.warnings.len(), 5, "{:?}", config.warnings);
    assert_eq!(config.sound.ultrasound(), Some(crate::gus::GusConfig::default()));
}

#[test]
fn mt32_and_host_midi_settings() {
    let text = "[sound]\nmidisynth=mt32\nmt32roms=~/roms\nmt32model=CM-32L\nmt32lib=lib/libmt32emu.so\n";
    let config = parse(text, Path::new("/cfg"), Some(Path::new("/home/u")));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let sound = &config.sound;
    assert_eq!(sound.midisynth, MidiSynth::Mt32);
    assert_eq!(sound.mt32roms.as_deref(), Some(Path::new("/home/u/roms")));
    assert_eq!(sound.mt32model, Mt32Model::Cm32l);
    assert_eq!(sound.mt32lib.as_deref(), Some(Path::new("/cfg/lib/libmt32emu.so")));

    let config = parse("[sound]\nmidisynth=host\nmidiport=UM-ONE\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!((config.sound.midisynth, config.sound.midiport.as_str()), (MidiSynth::Host, "UM-ONE"));

    let config = parse("[sound]\nmt32model=sc55\n", Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
    assert_eq!(config.sound.mt32model, Mt32Model::Auto);
}

#[test]
fn sound_canvas_settings() {
    let text = "[sound]\nmidisynth=SoundCanvas\nsc55roms=~/sc55\nsc55model=MK1-v1.21\n";
    let config = parse(text, Path::new("/cfg"), Some(Path::new("/home/u")));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let sound = &config.sound;
    assert_eq!(sound.midisynth, MidiSynth::Sc55);
    assert_eq!(sound.sc55roms.as_deref(), Some(Path::new("/home/u/sc55")));
    assert_eq!(sound.sc55model, "mk1-v1.21");

    let config = parse("[sound]\nsc55model=jv880\n", Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
    assert_eq!(config.sound.sc55model, "auto");
}

#[test]
fn the_builtin_ultrasound_drive() {
    // X:, where ULTRADIR points unless it is set.
    let gus = parse("", Path::new("/cfg"), None).sound.gus;
    assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "X:\\ULTRASND", true));
    let gus = parse("[sound]\ngusdrive=u:\n", Path::new("/cfg"), None).sound.gus;
    assert_eq!((gus.drive, gus.ultradir().as_str()), (Some(20), "U:\\ULTRASND"));
    let gus = parse("[sound]\ngusdrive=none\n", Path::new("/cfg"), None).sound.gus;
    assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (None, "C:\\ULTRASND", false));
    let gus = parse("[sound]\nultradir=D:\\GUS\n", Path::new("/cfg"), None).sound.gus;
    assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "D:\\GUS", false));

    for bad in ["C", "Z", "A:", "XY", ""] {
        let config = parse(&format!("[sound]\ngusdrive={}\n", bad), Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 1, "{}: {:?}", bad, config.warnings);
        assert_eq!(config.sound.gus.drive, Some(23));
    }

    // A drive of the user's own on X: wins.
    let config = parse("[sound]\n[drives]\nX=/games\n", Path::new("/cfg"), None);
    assert!(config.warnings[0].contains("gusdrive X:"), "{:?}", config.warnings);
    assert_eq!(config.sound.gus.ultradir(), "C:\\ULTRASND");
    let config = parse("[sound]\ngus=false\n[drives]\nX=/games\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

#[test]
fn ultrasound_conflicts_with_the_sound_blaster() {
    let config = parse("[sound]\ngusbase=220\n", Path::new("/cfg"), None);
    assert_eq!(config.sound.ultrasound(), None);
    assert!(config.warnings[0].contains("gusbase 220"), "{:?}", config.warnings);

    let config = parse("[sound]\ngusirq=7\ngusdma=1\n", Path::new("/cfg"), None);
    assert!(config.sound.ultrasound().is_some());
    assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);

    // Without a Sound Blaster, 220h is free.
    let config = parse("[sound]\nsbtype=none\ngusbase=220\n", Path::new("/cfg"), None);
    assert_eq!(config.sound.ultrasound().map(|g| g.base), Some(0x220));
}

#[test]
fn memsize_range() {
    let config = parse("[emulator]\nmemsize=32\n", Path::new("/cfg"), None);
    assert_eq!(config.memsize, Some(32));
    let config = parse("[emulator]\nmemsize=1024\n", Path::new("/cfg"), None);
    assert_eq!(config.memsize, None);
    assert!(config.warnings[0].contains("invalid memsize '1024'"));
}

#[test]
fn memsize_goes_as_far_as_the_cpu_takes() {
    let config = parse("[emulator]\ncpu=pentium_mmx\nmemsize=512\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(Settings::from_config(&config).memsize, 512);
    let config = parse("[emulator]\ncpu=pentium\nmemsize=512\n", Path::new("/cfg"), None);
    assert_eq!(config.warnings, ["memsize 512 MB is more than a Pentium takes: 256 MB"]);
    assert_eq!(Settings::from_config(&config).memsize, 256);
    // Without a cpu, the default 486's.
    let config = parse("[emulator]\nmemsize=256\n", Path::new("/cfg"), None);
    assert_eq!(Settings::from_config(&config).memsize, 128);
    let config = parse("[emulator]\ncpu=386\nmemsize=128\n", Path::new("/cfg"), None);
    assert_eq!(Settings::from_config(&config).memsize, 64);
}

#[test]
fn a_game_section_names_the_profile() {
    let config = parse("[game]\nname = Commander Keen 4\n[autoexec]\nKEEN4E\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.game_name.as_deref(), Some("Commander Keen 4"));
    let config = parse("[game]\nname=\nyear=1991\n", Path::new("/cfg"), None);
    assert_eq!((config.game_name, config.warnings.len()), (None, 2));
    let config = parse("[game]\noverlay=true\nmanual=a.pdf\nmanual=b.png|Map\n", Path::new("/cfg"), None);
    assert!(config.game_overlay && config.warnings.is_empty());
    assert_eq!(config.game_manuals, ["a.pdf", "b.png|Map"]);
}

#[test]
fn memory_settings() {
    let config = parse("[emulator]\nems=off\nUMB=no\ndpmi=false\n", Path::new("/cfg"), None);
    assert_eq!((config.ems, config.umb, config.dpmi), (Some(false), Some(false), Some(false)));
    let settings = Settings::from_config(&config);
    assert!(!settings.ems && !settings.umb && !settings.dpmi);
    let settings = Settings::from_config(&parse("", Path::new("/cfg"), None));
    assert!(settings.ems && settings.umb && settings.dpmi);
    let config = parse("[emulator]\nems=lots\n", Path::new("/cfg"), None);
    assert_eq!((config.ems, config.warnings.len()), (None, 1));
}

#[test]
fn dos_versions() {
    let version = |s: &str| DosVersion::parse(s).map(DosVersion::name);
    assert_eq!(version("5"), Some("5.00".to_string()));
    assert_eq!(version("5.0"), Some("5.00".to_string()));
    assert_eq!(version("7.1"), Some("7.10".to_string()));
    assert_eq!(version("7.10"), Some("7.10".to_string()));
    assert_eq!(version(" 6.22 "), Some("6.22".to_string()));
    for bad in ["", "1.0", "10", "7.", "7.100", "7.x", "seven"] {
        assert_eq!(version(bad), None, "{}", bad);
    }
    assert!(DosVersion::new(7, 10).at_least(7, 0) && !DosVersion::new(6, 22).at_least(7, 0));
    let config = parse("[emulator]\ndos_version=7.1\n", Path::new("/cfg"), None);
    assert_eq!(Settings::from_config(&config).dos_version, DosVersion::new(7, 10));
    assert_eq!(Settings::from_config(&parse("", Path::new("/cfg"), None)).dos_version, DosVersion::new(5, 0));
    let config = parse("[emulator]\ndos_version=12\n", Path::new("/cfg"), None);
    assert_eq!((config.dos_version, config.warnings.len()), (None, 1));
}

#[test]
fn cpu_model() {
    let config = parse("[emulator]\ncpu=386\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, Some(crate::cpu::CpuModel::I386));
    let config = parse("[emulator]\ncpu=Pentium\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, Some(crate::cpu::CpuModel::Pentium));
    let config = parse("[emulator]\ncpu=pentium_pro\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, Some(crate::cpu::CpuModel::PentiumPro));
    let config = parse("[emulator]\ncpu=pentium_ii\nmemsize=512\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, Some(crate::cpu::CpuModel::PentiumII));
    let config = parse("[emulator]\ncpu=pentium_iii\nmemsize=512\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, Some(crate::cpu::CpuModel::PentiumIII));
    assert!(config.warnings.is_empty());
    assert_eq!(Settings::from_config(&config).memsize, 512);
    let config = parse("[emulator]\ncpu=8086\n", Path::new("/cfg"), None);
    assert_eq!(config.cpu, None);
    assert!(config.warnings[0].contains("invalid cpu '8086'"));
}

#[test]
fn cycles_accepts_max_and_rejects_nonsense() {
    let config = parse("[emulator]\ncycles=Max\n", Path::new("/cfg"), None);
    assert_eq!(config.cycles, Some(CpuSpeed::Max));

    let config = parse("[emulator]\ncycles=fast\n", Path::new("/cfg"), None);
    assert_eq!(config.cycles, None);
    assert_eq!(config.warnings.len(), 1);
    assert!(config.warnings[0].starts_with("line 2: invalid cycles 'fast'"));
}

#[test]
fn core_takes_auto_dynamic_or_normal() {
    let config = parse("[emulator]\ncore=Dynamic\n", Path::new("/cfg"), None);
    assert_eq!(config.core, Some(CoreMode::Dynamic));
    assert_eq!(Settings::from_config(&config).core, CoreMode::Dynamic);
    assert_eq!(Settings::default().core, CoreMode::Auto);

    let config = parse("[emulator]\ncore=fast\n", Path::new("/cfg"), None);
    assert_eq!(config.core, None);
    assert!(config.warnings[0].starts_with("line 2: invalid core 'fast'"), "{:?}", config.warnings);
}

#[test]
fn fpu_takes_exact_or_fast() {
    let config = parse("[emulator]\nfpu=Fast\n", Path::new("/cfg"), None);
    assert_eq!(config.fpu_fast, Some(true));
    assert!(Settings::from_config(&config).fpu_fast);
    assert!(!Settings::default().fpu_fast);

    let config = parse("[emulator]\nfpu=sloppy\n", Path::new("/cfg"), None);
    assert_eq!(config.fpu_fast, None);
    assert!(config.warnings[0].starts_with("line 2: invalid fpu 'sloppy'"), "{:?}", config.warnings);
}

#[test]
fn template_is_all_comments() {
    let config = parse(TEMPLATE, Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert!(config.drives.is_empty());
    assert!(config.autoexec.is_empty());
    assert_eq!(config.scale, None);
    assert_eq!((config.cycles, config.core, config.fpu_fast), (None, None, None));
    assert_eq!((config.ems, config.umb), (None, None));
    assert_eq!((config.fullscreen, config.aspect, config.filter), (None, None, None));
    assert_eq!(config.vrr, None);
    assert_eq!((config.shader, config.monochrome, config.machine), (None, None, None));
    assert_eq!(config.sound, SoundConfig::default());
    assert_eq!(config.mixer, MixerSettings::default());
    assert_eq!(config.joystick, JoystickSettings::default());
}

#[test]
fn display_settings() {
    let text = "[emulator]\nfullscreen=yes\naspect=5:4\nvrr=on\nfilter=Linear\nshader=CRT\nmonochrome=Amber\ncrt_curvature=0\nCRT_Glow=45%\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!((config.fullscreen, config.aspect, config.filter), (Some(true), Some(AspectRatio::FiveFour), Some(Filter::Linear)));
    assert_eq!(config.vrr, Some(true));
    assert_eq!((config.shader, config.monochrome), (Some(Shader::Crt), Some(Monochrome::Amber)));
    assert_eq!(Settings::from_config(&config).crt, CrtSettings { curvature: 0, glow: 45 });
    for (name, ratio) in [
        ("none", AspectRatio::None), ("4:3", AspectRatio::FourThree), ("5:4", AspectRatio::FiveFour),
        ("16:10", AspectRatio::SixteenTen), ("16:9", AspectRatio::SixteenNine), ("true", AspectRatio::FourThree),
        ("false", AspectRatio::None), ("yes", AspectRatio::FourThree), ("0", AspectRatio::None),
    ] {
        let config = parse(&format!("[emulator]\naspect={}\n", name), Path::new("/cfg"), None);
        assert_eq!(config.aspect, Some(ratio));
    }
    let text = "[emulator]\nfullscreen=maybe\nvrr=sometimes\nfilter=blur\nshader=bent\nmonochrome=blue\ncrt_curvature=200\ncrt_glow=lots\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 7, "{:?}", config.warnings);
    assert!(config.warnings[1].contains("invalid vrr 'sometimes'"), "{:?}", config.warnings);
    assert!(config.warnings[6].contains("invalid crt_glow 'lots' (0 to 100)"), "{:?}", config.warnings);
    assert_eq!(Settings::from_config(&config), Settings::default());
}

#[test]
fn machine_settings() {
    let config = parse("[emulator]\nmachine=VGAonly\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.machine, Some(Adapter::Vga));
    assert_eq!(parse("[emulator]\nmachine=svga_s3\n", Path::new("/cfg"), None).machine, Some(Adapter::S3));
    assert_eq!(parse("[emulator]\nmachine=svga_et4000\n", Path::new("/cfg"), None).machine, Some(Adapter::Et4000));
    assert_eq!(parse("[emulator]\nmachine=svga_paradise\n", Path::new("/cfg"), None).machine, Some(Adapter::Svga));
    assert_eq!(parse("[emulator]\nmachine=PCjr\n", Path::new("/cfg"), None).machine, Some(Adapter::Pcjr));
    assert_eq!(parse("[emulator]\nmachine=tandy\n", Path::new("/cfg"), None).machine, Some(Adapter::Tandy));
    let config = parse("[emulator]\nmachine=mcga\n", Path::new("/cfg"), None);
    assert_eq!((config.machine, config.warnings.len()), (None, 1));
}

#[test]
fn composite_settings() {
    let config = parse("[emulator]\ncomposite=on\ncomposite_era=NEW\n", Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let settings = Settings::from_config(&config);
    assert_eq!(settings.composite, CompositeSettings { mode: CompositeMode::On, era: CompositeEra::New });
    let config = parse("[emulator]\ncomposite=rgb\ncomposite_era=1985\n", Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    assert_eq!(Settings::from_config(&config).composite, CompositeSettings::default());
}

#[test]
fn mixer_settings() {
    let text = "[mixer]\nmaster=80\nFM = 150%\ncdaudio=0\nsb=300\nbass=10\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    assert!(config.warnings[0].starts_with("line 5: sb: invalid volume '300'"), "{:?}", config.warnings);
    assert!(config.warnings[1].starts_with("line 6: unknown setting 'bass'"), "{:?}", config.warnings);
    let mixer = Settings::from_config(&config).mixer;
    let levels = Channel::ALL.map(|channel| mixer.level(channel));
    assert_eq!(levels, [80, 100, 100, 150, 100, 100, 0, 100, 100, 100, 100, 100, 100]);
    assert!(mixer.speaker_filter);
    assert_eq!((mixer.sb_filter, mixer.reverb, mixer.chorus), (SbFilter::Auto, ReverbPreset::Off, ChorusPreset::Off));

    assert_eq!((mixer.reverb_mix, mixer.chorus_mix), (50, 50));

    let text = "[mixer]\nspeaker_filter=off\nsb_filter=OFF\nreverb=Large\nchorus=light\nreverb_mix=80%\nChorus_Mix=0\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let mixer = config.mixer;
    assert!(!mixer.speaker_filter);
    assert_eq!((mixer.sb_filter, mixer.reverb, mixer.chorus), (SbFilter::Off, ReverbPreset::Large, ChorusPreset::Light));
    assert_eq!((mixer.reverb_mix, mixer.chorus_mix), (80, 0));
    let text = "[mixer]\nspeaker_filter=maybe\nsb_filter=sb1\nreverb=hall\nchorus=heavy\nreverb_mix=150\nchorus_mix=wet\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 6, "{:?}", config.warnings);
    assert!(config.warnings[4].contains("reverb_mix: invalid mix '150' (0 to 100)"), "{:?}", config.warnings);
}

#[test]
fn joystick_settings() {
    let text = "[Joystick]\njoysticktype=4AXIS\ndeadzone=25%\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.joystick, JoystickSettings { kind: JoystickType::FourAxis, deadzone: 25 });
    let text = "[joystick]\njoysticktype=fcs\ndeadzone=95\ntimed=false\n";
    let config = parse(text, Path::new("/cfg"), None);
    assert_eq!(config.warnings.len(), 3, "{:?}", config.warnings);
    assert_eq!(config.joystick, JoystickSettings::default());
}

#[test]
fn audio_buffer_parses_and_saves() {
    use crate::audio::AudioBuffer;
    assert_eq!(Settings::from_config(&parse("", Path::new("."), None)).audio_buffer, AudioBuffer::default());
    let config = parse("[mixer]\nblocksize=1024\nprebuffer=10ms\n", Path::new("."), None);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    let settings = Settings::from_config(&config);
    assert_eq!(settings.audio_buffer, AudioBuffer { blocksize: 1024, prebuffer: 10 });
    let saved = update_text("", &Settings::default(), &settings, &[], None);
    assert!(saved.contains("blocksize=1024") && saved.contains("prebuffer=10"), "{}", saved);
    assert_eq!(Settings::from_config(&parse(&saved, Path::new("."), None)).audio_buffer, settings.audio_buffer);
    for line in ["blocksize=500", "blocksize=4096", "blocksize=", "prebuffer=0", "prebuffer=12", "prebuffer=lots"] {
        let config = parse(&format!("[mixer]\n{}\n", line), Path::new("."), None);
        assert_eq!(config.warnings.len(), 1, "{}: {:?}", line, config.warnings);
        assert_eq!(Settings::from_config(&config).audio_buffer, AudioBuffer::default());
    }
}

#[test]
fn mouse_sensitivity_parses_and_saves() {
    assert_eq!(Settings::default().mouse_sensitivity, 1.0);
    assert_eq!(Settings::from_config(&parse("", Path::new("."), None)).mouse_sensitivity, 1.0);
    for value in [0.1, 1.0, 2.35, 5.0, 10.0] {
        let text = format!("[emulator]\nmouse_sensitivity={}\n", value);
        let config = parse(&text, Path::new("."), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let settings = Settings::from_config(&config);
        assert_eq!(settings.mouse_sensitivity, value);
        let saved = update_text("", &Settings::default(), &settings, &[], None);
        assert_eq!(Settings::from_config(&parse(&saved, Path::new("."), None)).mouse_sensitivity, value);
    }
    for value in ["0", "-1", "0.01", "0.099", "10.01", "NaN", "inf", "-inf", "fast", ""] {
        let config = parse(&format!("[emulator]\nmouse_sensitivity={}\n", value), Path::new("."), None);
        assert_eq!(config.warnings.len(), 1, "{}: {:?}", value, config.warnings);
        assert_eq!(Settings::from_config(&config).mouse_sensitivity, 1.0);
    }
}

#[test]
fn mouse_sensitivity_scales_motion_and_preserves_small_movements() {
    assert_eq!(Settings::default().mouse_motion(3.0, -6.0), (3.0, -6.0));
    let settings = Settings { mouse_sensitivity: 10.0, ..Settings::default() };
    assert_eq!(settings.mouse_motion(3.0, -6.0), (30.0, -60.0));
    let settings = Settings { mouse_sensitivity: 0.1, ..Settings::default() };
    let mut mouse = crate::mouse::MouseState::new();
    let before = (mouse.x, mouse.y);
    for _ in 0..100 {
        let (dx, dy) = settings.mouse_motion(0.3, 0.3);
        mouse.move_by(dx, dy);
    }
    assert_eq!((mouse.x - before.0, mouse.y - before.1), (3, 3));
}

/// Settings with every value away from its default.
fn changed_settings() -> Settings {
    let sound = SoundConfig {
        sb: crate::sb::SbConfig { model: crate::sb::SbModel::SbPro2, base: 0x240, irq: 5, dma8: 3, dma16: 6 },
        sb_installed: true,
        opl3: false,
        awe32rom: Some(PathBuf::from("/home/u/roms/awe32.raw")),
        awe32ram: 2048,
        soundfont: Some(PathBuf::from("/home/u/sf/General User.sf2")),
        gus: crate::gus::GusConfig {
            enabled: true,
            base: 0x260,
            irq: 11,
            dma: 6,
            drive: Some(20),
            ultradir: Some("D:\\GUS".to_string()),
        },
        midisynth: MidiSynth::Gus,
        mt32roms: Some(PathBuf::from("/home/u/roms")),
        mt32model: Mt32Model::Cm32l,
        mt32lib: Some(PathBuf::from("/opt/munt/libmt32emu.so")),
        sc55roms: Some(PathBuf::from("/home/u/sc55")),
        sc55model: "mk2".to_string(),
        midiport: "FLUID".to_string(),
        lpt_dac: LptDacType::Disney,
        tandy: crate::sn76489::TandySound::On,
        pit2_gate_at_start: true,
    };
    Settings {
        scale: 3,
        fullscreen: true,
        aspect: AspectRatio::FourThree,
        vrr: true,
        filter: Filter::Linear,
        shader: Shader::Crt,
        crt: CrtSettings { curvature: 70, glow: 90 },
        monochrome: Monochrome::Green,
        monitor_effects: crate::video::power::MonitorEffects::On,
        post: crate::post::PostMode::Full,
        composite: CompositeSettings { mode: CompositeMode::On, era: CompositeEra::New },
        machine: Adapter::Vga,
        voodoo: crate::voodoo::VoodooSettings {
            enabled: true,
            board: crate::voodoo::Board::Standard,
            renderer: crate::voodoo::Renderer::OpenGl,
            texture_sampling: crate::voodoo::TextureSampling::Unfiltered,
            scale: 3,
            scale_shader: false,
            msaa: 4,
            anisotropy: 8,
            fps_cap: Some(30),
            gamma: Some(1.5),
        },
        powervr: Some(crate::powervr::Chip::Pcx2),
        powervr_filter: crate::powervr::tsp::Filter::Point,
        capture_dir: PathBuf::from("/home/u/dos captures"),
        record_ui: true,
        record_shader: true,
        cycles: CpuSpeed::Fixed(3000),
        auto_fps: Some(35),
        cpu: CpuModel::I386,
        core: CoreMode::Dynamic,
        fpu_fast: true,
        memsize: 32,
        ems: false,
        umb: false,
        dpmi: false,
        dos_high: false,
        dos_version: DosVersion::new(7, 10),
        ide_hard_disks: false,
        boot_cdrom: false,
        floppy_controller: false,
        keyboard_layout: LayoutSetting::Named("gr"),
        mouse_autocapture: false,
        mouse_sensitivity: 2.3,
        mouse_capture_messages: false,
        mouse_menu_hold: false,
        version_check: false,
        rewind: true,
        rewind_memory: 512,
        idle_skip: false,
        idle_hint: true,
        sound,
        disk: DiskSettings {
            hard_disk_speed: DiskSpeed::Medium,
            floppy_disk_speed: DiskSpeed::Slow,
            hard_disk_noise: NoiseMode::On,
            floppy_disk_noise: NoiseMode::SeekOnly,
        },
        mixer: {
            let mut mixer = MixerSettings::default();
            for (i, channel) in Channel::ALL.into_iter().enumerate() {
                mixer.set_level(channel, 10 * i as u16 + 5);
            }
            mixer.speaker_filter = false;
            mixer.sb_filter = SbFilter::Off;
            mixer.reverb = ReverbPreset::Medium;
            mixer.chorus = ChorusPreset::Strong;
            mixer.reverb_mix = 70;
            mixer.chorus_mix = 20;
            mixer
        },
        audio_buffer: crate::audio::AudioBuffer { blocksize: 256, prebuffer: 10 },
        joystick: JoystickSettings { kind: JoystickType::TwoAxis, deadzone: 20 },
        network: crate::net::NetSettings {
            lan: Some("relay.example.com".into()),
            room: "doom".into(),
            ..Default::default()
        },
        serial: crate::serial::SerialSettings {
            ports: [
                crate::serial::PortType::Empty,
                crate::serial::PortType::NullModem,
                crate::serial::PortType::Mouse,
                crate::serial::PortType::Off,
            ],
            modem_listen: Some("0.0.0.0:2323".parse().unwrap()),
            ..Default::default()
        },
        printer: crate::printer::PrinterSettings {
            output: crate::printer::PrinterOutput::Png,
            paper: crate::printer::Paper::A4,
            docpath: PathBuf::from("/home/u/printouts"),
            device: Some("Office".into()),
            ..Default::default()
        },
        achievements: crate::achievements::AchievementSettings {
            enabled: true,
            username: "Player".to_string(),
            token: "abc123".to_string(),
            hardcore: true,
        },
        shell: crate::cmdline::settings::ShellSettings {
            autosuggest: false,
            history_size: 500,
            palette: crate::cmdline::settings::Palette {
                prompt: crate::cmdline::settings::DosColor(Some(14)),
                ..Default::default()
            },
            ..Default::default()
        },
        vr: crate::vr::VrSettings {
            mode: crate::vr::VrMode::Desktop,
            scene: Some(PathBuf::from("/home/u/rooms/den.glb")),
            controllers: crate::vr::VrControllers::Gamepad,
            spatial_audio: false,
            screen_fit: crate::vr::ScreenFit::Stretch,
            quality: crate::vr::VrQuality::Medium,
            screen_glow: 150,
            scene_scale: 104,
            seat: [-3, 2, 7],
            seat_turn: -5,
            graphics: crate::vr::VrGraphics::Vulkan,
            resolution: Some(80),
            ambient_occlusion: crate::vr::VrSwitch::Off,
            msaa: crate::vr::VrMsaa::Two,
            refresh: Some(90),
            foveation: crate::vr::VrFoveation::High,
            keyboard: crate::vr::VrKeyboard::Marked,
            time: crate::vr::VrTime::At(21 * 60 + 30),
            time_speed: 60,
            latitude: 52,
            weather: crate::vr::VrWeather::Snow,
            weather_sound: 30,
            lightning: false,
        },
    }
}

fn drive_specs() -> Vec<MountSpec> {
    use crate::disk::MountOptions;
    vec![
        MountSpec { drive: 2, path: "/home/u/dos".into(), opts: MountOptions::default() },
        MountSpec {
            drive: 3,
            path: "/home/u/cd images/game.cue".into(),
            opts: MountOptions { kind: DriveKind::CdRom, label: Some("GAME".into()), read_only: false, ..Default::default() },
        },
        MountSpec {
            drive: 0,
            path: "/home/u/disks/disk 1.img".into(),
            opts: MountOptions {
                kind: DriveKind::Floppy,
                more_images: vec!["/home/u/disks/disk 2.img".into()],
                ..Default::default()
            },
        },
    ]
}

#[test]
fn voodoo_anisotropy_defaults_to_off_and_respects_overrides() {
    assert_eq!(Settings::default().voodoo.anisotropy, 1);
    for (text, expected) in [
        ("[emulator]\n", 1),
        ("[emulator]\nvoodoo_anisotropy=off\n", 1),
        ("[emulator]\nvoodoo_anisotropy=4\n", 4),
    ] {
        let config = parse(text, Path::new("."), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config).voodoo.anisotropy, expected);
    }
}

#[test]
fn voodoo_texture_sampling_parsing_and_default() {
    use crate::voodoo::TextureSampling;
    assert_eq!(Settings::default().voodoo.texture_sampling, TextureSampling::Default);
    for (value, expected) in [("default", TextureSampling::Default), ("Unfiltered", TextureSampling::Unfiltered)] {
        let config = parse(&format!("[emulator]\nvoodoo_texture_sampling={}\n", value), Path::new("."), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config).voodoo.texture_sampling, expected);
    }
    let config = parse("[emulator]\nvoodoo_texture_sampling=invalid\n", Path::new("."), None);
    assert_eq!(config.warnings.len(), 1);
    assert_eq!(Settings::from_config(&config).voodoo.texture_sampling, TextureSampling::Default);
}

#[test]
fn saved_settings_parse_back() {
    let home = Path::new("/home/u");
    let settings = changed_settings();
    let changes: Vec<DriveChange> = drive_specs().into_iter().map(|s| (s.drive, Some(s))).collect();
    let text = update_text(TEMPLATE, &Settings::default(), &settings, &changes, Some(home));
    let config = parse(&text, Path::new("/cfg"), Some(home));
    assert!(config.warnings.is_empty(), "{:?}\n{}", config.warnings, text);
    assert_eq!(Settings::from_config(&config), settings);
    assert_eq!(config.drives, drive_specs());

    // The template's examples and comments stay, each new line right
    // after its example.
    for line in TEMPLATE.lines() {
        assert!(text.contains(line), "lost '{}'", line);
    }
    assert!(text.contains("#scale=2\nscale=3\n"), "{}", text);
    assert!(text.contains("#shader=none\nshader=crt\n"), "{}", text);
    assert!(text.contains("#monochrome=off\nmonochrome=green\n"), "{}", text);
    assert!(text.contains("#machine=svga\nmachine=vga\n"), "{}", text);
    assert!(text.contains("#voodoo_texture_sampling=default\nvoodoo_texture_sampling=unfiltered\n"), "{}", text);
    assert!(text.contains("#capture_dir=capture\ncapture_dir=~/dos captures\n"), "{}", text);
    assert!(text.contains("#record_ui=false\nrecord_ui=true\n"), "{}", text);
    assert!(text.contains("#record_shader=false\nrecord_shader=true\n"), "{}", text);
    assert!(text.contains("#master=100\nmaster=5\n"), "{}", text);
    assert!(text.contains("#disknoise=100\ndisknoise=75\n"), "{}", text);
    assert!(text.contains("#joysticktype=auto\njoysticktype=2axis\n"), "{}", text);
    assert!(text.contains("#sbtype=sb16\nsbtype=sbpro2\n"), "{}", text);
    assert!(text.contains("#E=~/dos/images/game.cue\nC=~/dos\nD=\"~/cd images/game.cue\" cdrom -label GAME\n"), "{}", text);
    assert!(text.contains("soundfont=~/sf/General User.sf2\n"), "{}", text);

    // Saving the same again changes nothing.
    assert_eq!(update_text(&text, &settings, &settings, &changes, Some(home)), text);
}

#[test]
fn unchanged_settings_leave_the_file_alone() {
    let settings = changed_settings();
    assert_eq!(update_text(TEMPLATE, &settings, &settings, &[], None), TEMPLATE);
    assert_eq!(update_text("", &settings, &settings, &[], None), "");
    assert_eq!(update_text("[emulator]\nscale=2", &settings, &settings, &[], None), "[emulator]\nscale=2");
}

#[test]
fn existing_lines_change_in_place() {
    let text = "\u{FEFF}[Emulator]\r\nScale = 2\r\ncycles=max\r\n[sound]\r\nsoundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /old\r\n# keep\r\nD=/cd cdrom\r\nd=/cd2\r\n[autoexec]\r\nC:\r\n";
    let sound = SoundConfig { soundfont: Some("gm.sf2".into()), ..SoundConfig::default() };
    let baseline = Settings { scale: 2, sound, ..Settings::default() };
    let settings = Settings { scale: 4, ..Settings::default() };
    let spec = |drive, path: &str| Some(MountSpec { drive, path: path.into(), opts: Default::default() });
    let changes = [(2, spec(2, "/new")), (3, None), (4, spec(4, "/e"))];
    assert_eq!(
        update_text(text, &baseline, &settings, &changes, None),
        // A cleared setting is commented out; a removed drive loses all
        // its lines; a new one goes after the other drives.
        "\u{FEFF}[Emulator]\r\nScale = 4\r\ncycles=max\r\n[sound]\r\n#soundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /new\r\nE=/e\r\n# keep\r\n[autoexec]\r\nC:\r\n"
    );
}

#[test]
fn disks_mounted_by_number_are_saved_by_number() {
    use crate::disk::numbered_drive;
    let settings = Settings::default();
    let spec = |drive, path: &str| Some(MountSpec { drive, path: path.into(), opts: Default::default() });
    let text = "[drives]\nC=/c\n";
    let added = update_text(text, &settings, &settings, &[(numbered_drive(2), spec(numbered_drive(2), "/os.vhd"))], None);
    assert_eq!(added, "[drives]\nC=/c\n2=/os.vhd\n");
    let changed = update_text(&added, &settings, &settings, &[(numbered_drive(2), spec(numbered_drive(2), "/w98.vhd"))], None);
    assert_eq!(changed, "[drives]\nC=/c\n2=/w98.vhd\n");
    assert_eq!(update_text(&changed, &settings, &settings, &[(numbered_drive(2), None)], None), text);
    assert_eq!(parse(&changed, Path::new("/cfg"), None).drives[1].drive, numbered_drive(2));
}

#[test]
fn the_autoexec_section_is_edited_as_written() {
    let text = "\u{FEFF}[emulator]\r\nscale=2\r\n[AUTOEXEC]\r\n# mine\r\nMOUNT D ~/d\r\n\r\nD:\r\n\r\n[sound]\r\nopl=opl3\r\n[autoexec]\r\nDIR\r\n";
    assert_eq!(autoexec_lines(text), ["# mine", "MOUNT D ~/d", "", "D:", "DIR"]);

    // In the first section, the header as written, the later one gone.
    let lines = ["# mine".to_string(), "C:".to_string(), String::new()];
    assert_eq!(
        with_autoexec(text, &lines),
        "\u{FEFF}[emulator]\r\nscale=2\r\n[AUTOEXEC]\r\n# mine\r\nC:\r\n\r\n[sound]\r\nopl=opl3\r\n"
    );
    // Emptied, it stays; without one, it goes at the end.
    assert_eq!(with_autoexec("[autoexec]\nDIR\n", &[]), "[autoexec]\n");
    assert_eq!(with_autoexec("[emulator]\nscale=2\n", &["DIR".to_string()]), "[emulator]\nscale=2\n\n[autoexec]\nDIR\n");
    assert_eq!(with_autoexec("", &["DIR".to_string()]), "[autoexec]\nDIR\n");

    // The template's commented examples are there to edit.
    assert_eq!(autoexec_lines(TEMPLATE).last().map(String::as_str), Some("#CD GAMES"));
}

#[test]
fn missing_sections_go_before_autoexec() {
    let settings = Settings { memsize: 8, ..Settings::default() };
    let base = Settings::default();
    let changes = [(3, Some(MountSpec { drive: 3, path: "/d".into(), opts: Default::default() }))];
    assert_eq!(
        update_text("[autoexec]\nDIR\n", &base, &settings, &changes, None),
        "[emulator]\nmemsize=8\n\n[drives]\nD=/d\n\n[autoexec]\nDIR\n"
    );
    assert_eq!(update_text("# mine\n", &base, &settings, &[], None), "# mine\n\n[emulator]\nmemsize=8\n");
    // A second [emulator] block gets the new line.
    assert_eq!(
        update_text("[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\n", &base, &settings, &[], None),
        "[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\nmemsize=8\n"
    );
}

#[test]
fn save_replaces_the_file() {
    let base = scratch("save");
    let path = base.join(FILE_NAME);
    fs::write(&path, "# my settings\n[emulator]\nscale=2\n").unwrap();
    let before = Settings { scale: 2, ..Settings::default() };
    let settings = Settings { scale: 3, ..Settings::default() };
    save(&path, &before, &settings, &[], None, Saving::Changes).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "# my settings\n[emulator]\nscale=3\n");
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);

    // A file that has gone missing starts over from the template.
    fs::remove_file(&path).unwrap();
    save(&path, &before, &settings, &[], None, Saving::Changes).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("#scale=2\nscale=3\n"));

    // All of them: the file gets every setting, and parses back to them.
    fs::write(&path, "# my settings\n[emulator]\nscale=2\n").unwrap();
    save(&path, &before, &settings, &[], None, Saving::All).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my settings\n[emulator]\nscale=3\nfullscreen=false\n"), "{}", text);
    assert!(text.contains("\n[mixer]\nmaster=100\n"), "{}", text);
    assert_eq!(Settings::from_config(&parse(&text, &base, None)), settings);
}

#[test]
fn saving_all_fills_in_every_setting() {
    let home = Path::new("/home/u");
    let text = "[emulator]\nscale = 2\ncycles=max\n[sound]\nsoundfont=sf/gm.sf2\n\n[autoexec]\nDIR\n";
    let file = Settings::from_config(&parse(text, Path::new("/cfg"), Some(home)));
    assert_eq!(file.sound.soundfont.as_deref(), Some(Path::new("/cfg/sf/gm.sf2")));
    // The command line asked for more speed, and the window for a
    // shader and no Gravis Ultrasound.
    let baseline = Settings { cycles: CpuSpeed::Fixed(20000), ..file.clone() };
    let mut settings = Settings { shader: Shader::Aperture, ..baseline.clone() };
    settings.sound.gus.enabled = false;
    let saved = complete_text(text, Path::new("/cfg"), &baseline, &settings, &[], Some(home));

    // Every setting has a line, the lines that were there stay as they
    // were written, and the command line's speed isn't kept.
    for (section, key, _) in entries(&settings, Some(home)) {
        // (The file's SoundFont; no Ultrasound directory, MT-32 or
        // Sound Canvas, LAN password, RetroAchievements account or
        // printer paths and programs of its own.)
        let wanted = !matches!(
            key,
            "ultradir" | "awe32rom" | "mt32roms" | "mt32lib" | "sc55roms" | "midiport" | "password" | "username" | "token"
                | "docpath" | "fontpath" | "device" | "print_command" | "open_with" | "scene"
        );
        assert_eq!(has_key(&saved.lines().map(str::to_string).collect::<Vec<_>>(), section, key), wanted, "{}\n{}", key, saved);
    }
    assert!(saved.starts_with("[emulator]\nscale = 2\ncycles=max\nfullscreen=false\n"), "{}", saved);
    assert!(saved.contains("shader=aperture\n") && saved.contains("gus=false\n"), "{}", saved);
    assert!(saved.contains("soundfont=sf/gm.sf2\n"), "{}", saved);
    assert!(saved.contains("\n[joystick]\njoysticktype=auto\ndeadzone=10\n\n[network]\nipx=auto\n"), "{}", saved);
    assert!(saved.contains("\nroom=lobby\n\n[serial]\nserial1=mouse\n"), "{}", saved);
    assert!(saved.contains("\nmodemtelnet=off\n\n[printer]\noutput=pdf\n"), "{}", saved);
    assert!(saved.contains("\ntimeout=3000\n\n[achievements]\nenabled=false\nhardcore=false\n\n[shell]\nautosuggest=true\n"), "{}", saved);
    assert!(saved.contains("\nsuggestion_color=darkgray\n\n[vr]\nmode=off\ncontrollers=both\nspatial_audio=true\nscreen_fit=auto\nquality=auto\nscreen_glow=100\nscene_scale=100\nseat_right=0\nseat_up=0\nseat_forward=0\nseat_turn=0\ngraphics=auto\nresolution=auto\nambient_occlusion=auto\nmsaa=auto\nrefresh=auto\nfoveation=auto\nkeyboard=off\ntime=now\ntime_speed=1\nlatitude=48\nweather=clear\nweather_sound=70\nlightning=true\n\n[autoexec]\nDIR\n"), "{}", saved);
    let config = parse(&saved, Path::new("/cfg"), Some(home));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(Settings::from_config(&config), Settings { cycles: CpuSpeed::Max, ..settings.clone() });

    // Saving again changes nothing; a speed the window changed is saved.
    assert_eq!(complete_text(&saved, Path::new("/cfg"), &settings, &settings, &[], Some(home)), saved);
    let faster = Settings { cycles: CpuSpeed::Fixed(30000), ..settings.clone() };
    let again = complete_text(&saved, Path::new("/cfg"), &settings, &faster, &[], Some(home));
    assert!(again.starts_with("[emulator]\nscale = 2\ncycles=30000\n"), "{}", again);
}

#[test]
fn load_writes_template_once_and_reads_local_files() {
    let base = scratch("load");
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).unwrap();
    let default = base.join("cfg/rust-dos/rust-dos.conf");

    let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
    assert!(config.created);
    assert_eq!(config.source.as_deref(), Some(default.as_path()));
    assert_eq!(fs::read_to_string(&default).unwrap(), TEMPLATE);

    // Second start uses the (edited) file and doesn't rewrite it
    fs::write(&default, "[emulator]\nscale=2\n").unwrap();
    let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
    assert!(!config.created);
    assert_eq!(config.scale, Some(2));

    // A file in the working directory wins, with paths relative to it
    fs::write(cwd.join(FILE_NAME), "[drives]\nA=disks/a floppy\nscale=9\n").unwrap();
    let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
    assert_eq!(config.scale, None);
    assert_eq!(config.drive(0).unwrap().path, cwd.join("disks/a"));
    assert_eq!(config.warnings.len(), 1);
    assert!(config.warnings[0].contains("rust-dos.conf: line 3"));

    assert!(load(Some(Path::new("nope.conf")), &cwd, None, Some(default), None).is_err());
}

#[test]
fn unwritable_default_is_only_a_warning() {
    let base = scratch("unwritable");
    let blocker = base.join("file");
    fs::write(&blocker, "").unwrap();
    // The parent "directory" is a file, so creating it fails
    let config = load(None, &base, None, Some(blocker.join("rust-dos.conf")), None).unwrap();
    assert!(!config.created);
    assert_eq!(config.warnings.len(), 1);
    assert!(config.drives.is_empty());
}
