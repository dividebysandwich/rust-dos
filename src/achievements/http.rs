//! The site over HTTPS: requests go to a thread of their own, which posts
//! them to dorequest.php one after another, and the answers come back.

use super::client::{Request, Transport};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

const URL: &str = "https://retroachievements.org/dorequest.php";

pub struct HttpTransport {
    requests: Sender<Request>,
    answers: Receiver<(u64, Result<String, String>)>,
}

impl HttpTransport {
    pub fn start() -> Self {
        let (requests, queue) = channel::<Request>();
        let (done, answers) = channel();
        std::thread::Builder::new()
            .name("retroachievements".to_string())
            .spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(Duration::from_secs(30)))
                    // The site's errors are JSON with a status, to read.
                    .http_status_as_error(false)
                    .user_agent(user_agent())
                    .build()
                    .into();
                for request in queue {
                    let answer = agent
                        .post(URL)
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .send(request.body())
                        .and_then(|mut response| response.body_mut().read_to_string())
                        .map_err(|e| e.to_string());
                    if done.send((request.id, answer)).is_err() {
                        break;
                    }
                }
            })
            .expect("the RetroAchievements thread");
        Self { requests, answers }
    }
}

/// What the site knows the emulator by.
fn user_agent() -> String {
    format!(
        "rust-dos/{} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

impl Transport for HttpTransport {
    fn send(&mut self, request: Request) {
        let _ = self.requests.send(request);
    }

    fn poll(&mut self) -> Vec<(u64, Result<String, String>)> {
        self.answers.try_iter().collect()
    }
}
