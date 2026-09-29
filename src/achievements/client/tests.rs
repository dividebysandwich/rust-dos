use super::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The site as the tests answer it: requests sent, and answers to give.
#[derive(Default)]
struct Site {
    sent: Vec<Request>,
    answers: Vec<(u64, Result<String, String>)>,
}

struct FakeTransport(Rc<RefCell<Site>>);

impl Transport for FakeTransport {
    fn send(&mut self, request: Request) {
        self.0.borrow_mut().sent.push(request);
    }

    fn poll(&mut self) -> Vec<(u64, Result<String, String>)> {
        std::mem::take(&mut self.0.borrow_mut().answers)
    }
}

fn field<'a>(request: &'a Request, key: &str) -> &'a str {
    request
        .fields
        .iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

/// Answer the last request sent (which must be `api`) with `body`.
fn answer(site: &Rc<RefCell<Site>>, achievements: &mut Achievements, api: &str, body: &str) {
    let id = {
        let site = site.borrow();
        let last = site.sent.last().expect("a request");
        assert_eq!(field(last, "r"), api, "{:?}", last);
        last.id
    };
    site.borrow_mut().answers.push((id, Ok(body.to_string())));
    achievements.poll();
}

fn settings() -> AchievementSettings {
    AchievementSettings {
        enabled: true,
        username: "player".into(),
        token: "tok".into(),
        hardcore: false,
    }
}

const GAME: &str = r#"{"Success":true,"GameId":42,"Title":"Keen","ConsoleId":26,"RichPresencePatch":"Display:\nLevel @Number(0xH0000)",
    "Sets":[{"AchievementSetId":1,"GameId":42,"Title":null,"Type":"core","ImageIconUrl":"",
      "Achievements":[
        {"ID":10,"Title":"First","Description":"Byte 0 is 1","Flags":3,"Points":5,"MemAddr":"0xH0000=1","BadgeName":"00001","Type":""},
        {"ID":11,"Title":"Second","Description":"Byte 1 is 1","Flags":3,"Points":10,"MemAddr":"0xH0001=1","BadgeName":"00002","Type":"progression"},
        {"ID":12,"Title":"[m] Draft","Description":"Unofficial","Flags":5,"Points":1,"MemAddr":"0xH0002=1","BadgeName":"00003","Type":""},
        {"ID":13,"Title":"Broken","Description":"","Flags":3,"Points":1,"MemAddr":"0xH0002=","BadgeName":"00003","Type":""}],
      "Leaderboards":[{"ID":7,"Title":"Fast","Description":"Quick","Mem":"STA:0xH0003=1::CAN:0xH0003=2::SUB:0xH0003=3::VAL:0xH0004","Format":"VALUE","LowerIsBetter":true,"Hidden":false}]}]}"#;

#[test]
fn a_session_logs_in_loads_the_game_and_unlocks() {
    let site = Rc::new(RefCell::new(Site::default()));
    let mut ra = Achievements::new(Box::new(FakeTransport(site.clone())), settings());
    // Logging in with the token at the start.
    assert_eq!(field(site.borrow().sent.last().unwrap(), "t"), "tok");
    answer(
        &site,
        &mut ra,
        "login2",
        r#"{"Success":true,"User":"Player","Token":"tok2","Score":100,"SoftcoreScore":3}"#,
    );
    assert!(ra.logged_in());
    assert_eq!(ra.take_new_token(), Some(("Player".into(), "tok2".into())));

    // A game with a hash: its sets, then its session.
    let hash = "0123456789abcdef0123456789abcdef".to_string();
    ra.game_started(Some(hash.clone()), "Commander Keen");
    {
        let site = site.borrow();
        let request = site.sent.last().unwrap();
        assert_eq!(
            (
                field(request, "m"),
                field(request, "u"),
                field(request, "t")
            ),
            (hash.as_str(), "Player", "tok2")
        );
    }
    answer(&site, &mut ra, "achievementsets", GAME);
    assert!(
        ra.take_notices()
            .iter()
            .any(|n| n.detail.contains("can't be read")),
        "the broken definition"
    );
    answer(
        &site,
        &mut ra,
        "startsession",
        r#"{"Success":true,"Unlocks":[{"ID":11,"When":1}],"HardcoreUnlocks":[]}"#,
    );
    let GameState::Playing(game) = &ra.game else {
        panic!("playing")
    };
    assert_eq!(game.progress(), (1, 2, 10, 15));
    assert_eq!(game.achievements[2].title, "Draft");
    assert!(ra.status().contains("1 of 2 unlocked"));
    ra.take_notices();

    // Byte 0 goes from 0 to 1: unlocked, and sent signed.
    let mut ram = vec![0u8; 0x20_0000];
    ra.do_frame(&ram, true);
    ram[0] = 1;
    ra.do_frame(&ram, true);
    let notices = ra.take_notices();
    assert_eq!(notices[0].title, "Achievement unlocked: First");
    assert!(
        notices.iter().any(|n| n.title == "Completed Keen"),
        "{:?}",
        notices
    );
    {
        let site = site.borrow();
        let award = site.sent.last().unwrap();
        assert_eq!(
            (field(award, "r"), field(award, "a"), field(award, "h")),
            ("awardachievement", "10", "0")
        );
        assert_eq!(field(award, "v"), md5_hex("10Player0"));
    }
    answer(
        &site,
        &mut ra,
        "awardachievement",
        r#"{"Success":true,"Score":105,"SoftcoreScore":3,"AchievementID":10}"#,
    );
    assert!(matches!(ra.user, UserState::LoggedIn { score: 105, .. }));

    // An unofficial one shows, and isn't sent. Leaderboards are hardcore's.
    let before = site.borrow().sent.len();
    ram[2] = 1;
    ram[3] = 1;
    ra.do_frame(&ram, true);
    assert_eq!(ra.take_notices()[0].title, "Unofficial achievement: Draft");
    assert_eq!(site.borrow().sent.len(), before);
}

#[test]
fn hardcore_leaderboards_retries_and_unknown_games() {
    let site = Rc::new(RefCell::new(Site::default()));
    let mut ra = Achievements::new(
        Box::new(FakeTransport(site.clone())),
        AchievementSettings {
            hardcore: true,
            ..settings()
        },
    );
    answer(
        &site,
        &mut ra,
        "login2",
        r#"{"Success":true,"User":"player","Token":"tok"}"#,
    );
    assert_eq!(ra.take_new_token(), None, "the same token");
    ra.game_started(Some("aa".repeat(16)), "Keen");
    answer(&site, &mut ra, "achievementsets", GAME);
    assert_eq!(field(site.borrow().sent.last().unwrap(), "h"), "1");
    answer(
        &site,
        &mut ra,
        "startsession",
        r#"{"Success":true,"Unlocks":[{"ID":11,"When":1}],"HardcoreUnlocks":[]}"#,
    );
    assert!(ra.hardcore_active());
    let GameState::Playing(game) = &ra.game else {
        panic!("playing")
    };
    assert_eq!(
        game.progress().0,
        0,
        "softcore unlocks don't count in hardcore"
    );

    let mut ram = vec![0u8; 0x20_0000];
    ra.do_frame(&ram, true);
    ram[3] = 1;
    ram[4] = 9;
    ra.do_frame(&ram, true);
    let GameState::Playing(game) = &ra.game else {
        panic!("playing")
    };
    assert_eq!(game.trackers(), ["9"]);
    ram[3] = 3;
    ra.do_frame(&ram, true);
    let submit = site.borrow().sent.last().unwrap().clone();
    assert_eq!(
        (
            field(&submit, "r"),
            field(&submit, "i"),
            field(&submit, "s")
        ),
        ("submitlbentry", "7", "9")
    );
    assert_eq!(field(&submit, "v"), md5_hex("7player9"));
    // The network failed: it goes again.
    site.borrow_mut()
        .answers
        .push((submit.id, Err("connection reset".into())));
    ra.poll();
    ra.retries[0].at = Instant::now();
    ra.poll();
    assert_eq!(
        field(site.borrow().sent.last().unwrap(), "r"),
        "submitlbentry"
    );
    answer(
        &site,
        &mut ra,
        "submitlbentry",
        r#"{"Success":true,"Response":{"Score":9,"BestScore":9,"RankInfo":{"Rank":2,"NumEntries":"5"},"TopEntries":[]}}"#,
    );
    assert!(ra.take_notices().iter().any(|n| n.detail == "Rank 2 of 5"));

    // A game the site doesn't know.
    ra.game_ended();
    ra.game_started(Some("bb".repeat(16)), "Mystery");
    answer(
        &site,
        &mut ra,
        "achievementsets",
        r#"{"Success":false,"Error":"Unknown game","Code":"not_found","Status":404}"#,
    );
    assert!(matches!(ra.game, GameState::Unknown { .. }));
    ra.game_started(None, "Nameless");
    assert!(matches!(ra.game, GameState::NoHash));
}

#[test]
fn a_refused_token_is_forgotten_but_not_for_an_error_page() {
    let site = Rc::new(RefCell::new(Site::default()));
    let mut ra = Achievements::new(Box::new(FakeTransport(site.clone())), settings());
    answer(&site, &mut ra, "login2", "<html>502 Bad Gateway</html>");
    assert!(matches!(ra.user, UserState::Failed(_)));
    assert_eq!(ra.take_new_token(), None);
    let mut ra = Achievements::new(Box::new(FakeTransport(site.clone())), settings());
    answer(
        &site,
        &mut ra,
        "login2",
        r#"{"Success":false,"Error":"Invalid token","Code":"invalid_credentials"}"#,
    );
    assert_eq!(ra.take_new_token(), Some(("player".into(), String::new())));
    // With the password.
    ra.login("Someone", "secret");
    assert_eq!(field(site.borrow().sent.last().unwrap(), "p"), "secret");
    let request = site.borrow().sent.last().unwrap().clone();
    assert_eq!(request.body(), "r=login2&u=Someone&p=secret");
    assert_eq!(
        Request {
            id: 0,
            fields: vec![("m".into(), "Level 1: 50%".into())]
        }
        .body(),
        "m=Level%201%3A%2050%25"
    );
}

#[test]
fn settings_read_and_write() {
    let mut s = AchievementSettings::default();
    s.set("enabled", "on").unwrap();
    s.set("hardcore", "true").unwrap();
    s.set("username", " Player ").unwrap();
    assert!(s.set("hardcore", "maybe").is_err() && s.set("colour", "red").is_err());
    assert_eq!(s.entries()[2], ("username", Some("Player".into())));
    assert_eq!(s.entries()[3], ("token", None));
}
