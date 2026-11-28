//! `rust-dos report`: a report to the almanac (almanac.rs) from the
//! command line, as the settings window's "Report settings to Almanac"
//! sends one. The settings are the ones rust-dos would start with: the
//! configuration file's, the machine options before `report`, and the
//! profile `--game` names over them. The program's file on the host says
//! which game and version the report is of.

use crate::args::{Args, ReportArgs};
use crate::config_file::{find_game, load_config};
use crate::startup::apply_args;
use rust_dos::almanac::{self, About, Subject};
use rust_dos::config::Settings;
use rust_dos::games;

/// Send the report; the process's exit code.
pub(crate) fn report(args: &Args, report: ReportArgs) -> i32 {
    match send(args, report) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("rust-dos report: {}", e);
            1
        }
    }
}

fn send(args: &Args, report: ReportArgs) -> Result<(), String> {
    let home = rust_dos::hostdirs::home_dir();
    let config = load_config(args)?;
    let mut settings = Settings::from_config(&config);
    apply_args(&mut settings, args)?;

    // A profile's settings over the configuration's, and what it says of
    // its game, as when it is launched.
    let profile = match &args.game {
        Some(query) => Some(find_game(config.source.as_deref(), query)?),
        None => None,
    };
    let mut about = About::default();
    let mut name = report.name.clone();
    if let Some((entry, text, dir)) = &profile {
        settings = games::prepare(&entry.id, &settings, text, dir, home.as_deref())?.settings;
        about = About::of_profile(text);
        name = name.or_else(|| Some(entry.name.clone()));
    }
    about.year = report.year.or(about.year);
    // On one line, as the settings window's dialog takes them: a line
    // break would end the profile's `notes=` line and start another.
    if let Some(publisher) = &report.publisher {
        about.publisher = games::one_line(publisher);
    }
    if let Some(notes) = &report.notes {
        about.notes = games::one_line(notes);
    }
    let key = match &report.key {
        Some(key) if games::valid_uuid(key) => Some(key.to_ascii_lowercase()),
        Some(key) => return Err(format!("--key {} isn't a UUID", key)),
        None => None,
    };

    let bytes = std::fs::read(&report.program)
        .map_err(|e| format!("{}: {}", report.program.display(), e))?;
    let file = almanac::file_info(&report.program.to_string_lossy(), &bytes);
    let platform =
        (!almanac::is_installer(&file.name)).then(|| almanac::platform_of(&bytes).to_string());
    let files = vec![file];

    let candidates = almanac::matches(&files, name.as_deref().unwrap_or(""), name.is_some())
        .map_err(limit_hint)?;
    let subject = almanac::subject_for(&candidates, name.as_deref()).ok_or_else(|| {
        format!(
            "{} doesn't know {}: give the game's name with --name",
            almanac::SITE,
            files[0].name.to_ascii_uppercase()
        )
    })?;

    // A profile keeps the year, publisher and notes, and a key, as the
    // settings window has it keep them: the key given if it has none of
    // its own. The report goes with the key given, else the profile's,
    // else a new one, printed. A dry run changes nothing.
    if let Some((entry, text, dir)) = profile.as_ref().filter(|_| !report.dry_run) {
        let with_key = match &key {
            Some(key) if About::of_profile(text).key.is_none() => {
                games::set_game_value(text, "uuid", Some(key))
            }
            _ => text.clone(),
        };
        let kept = about.into_profile(&with_key);
        if kept != *text {
            let path = dir.join(format!("{}.conf", entry.id));
            match std::fs::write(&path, &kept) {
                Ok(()) => about = About::of_profile(&kept),
                Err(e) => eprintln!(
                    "rust-dos report: the profile can't keep the year, publisher, notes \
                     and key: {}: {}",
                    path.display(),
                    e
                ),
            }
        }
    }
    let mut new_key = false;
    if key.is_some() {
        about.key = key;
    } else if about.key.is_none() && !report.dry_run {
        about.key = games::new_uuid();
        new_key = about.key.is_some();
    }
    let settings = almanac::report_settings(&settings, settings.memory_mb(), home.as_deref());
    if report.dry_run {
        match &subject {
            Subject::Game { slug, version } => {
                let version =
                    version.map_or("a new version".to_string(), |v| format!("version {}", v));
                println!(
                    "Would report to {} ({}), {}",
                    slug,
                    version,
                    report.rating.label()
                );
            }
            Subject::New(name) => {
                println!("Would add the game {}, {}", name, report.rating.label())
            }
        }
        for (section, values) in &settings {
            println!("[{}]", section);
            for (key, value) in values {
                println!("{}={}", key, value);
            }
        }
        return Ok(());
    }
    let reported = almanac::report(
        &subject,
        &files,
        platform.as_deref(),
        report.rating,
        &settings,
        &about,
    )
    .map_err(limit_hint)?;

    let done = if reported.replaced {
        "replaces the one sent with its key before"
    } else {
        "is in the almanac"
    };
    println!(
        "Report #{} ({}) {}: https://{}/games/{}",
        reported.id,
        report.rating.label(),
        done,
        almanac::SITE,
        reported.slug
    );
    if let Some(key) = about.key.as_deref().filter(|_| new_key) {
        println!(
            "Its key is {}: report again with --key {} to replace it",
            key, key
        );
    }
    Ok(())
}

/// The almanac's error `e`, with how to lift its limit on an address if
/// that is what turned the request down.
fn limit_hint(e: String) -> String {
    if e.contains("too many requests") {
        format!(
            "{} (an almanac staff token in {} lifts the limit)",
            e,
            almanac::TOKEN_VARIABLE
        )
    } else {
        e
    }
}
