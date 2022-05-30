use clap::Parser;
use collab::{Error, Result};
use db::{Db, PostgresDb, UserId};
use rand::prelude::*;
use serde::Deserialize;
use std::fmt::Write;
use time::{Duration, OffsetDateTime};

#[allow(unused)]
#[path = "../db.rs"]
mod db;

#[derive(Parser)]
struct Args {
    /// Seed users from GitHub.
    #[clap(short, long)]
    github_users: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    id: usize,
    login: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut rng = StdRng::from_entropy();
    let database_url = std::env::var("DATABASE_URL").expect("missing DATABASE_URL env var");
    let db = PostgresDb::new(&database_url, 5)
        .await
        .expect("failed to connect to postgres database");

    let mut zed_users = vec![
        ("nathansobo".to_string(), Some("nathan@zed.dev")),
        ("maxbrunsfeld".to_string(), Some("max@zed.dev")),
        ("as-cii".to_string(), Some("antonio@zed.dev")),
        ("iamnbutler".to_string(), Some("nate@zed.dev")),
        ("gibusu".to_string(), Some("greg@zed.dev")),
        ("Kethku".to_string(), Some("keith@zed.dev")),
    ];

    if args.github_users {
        let github_token = std::env::var("GITHUB_TOKEN").expect("missing GITHUB_TOKEN env var");
        let client = reqwest::Client::new();
        let mut last_user_id = None;
        for page in 0..20 {
            println!("Downloading users from GitHub, page {}", page);
            let mut uri = "https://api.github.com/users?per_page=100".to_string();
            if let Some(last_user_id) = last_user_id {
                write!(&mut uri, "&since={}", last_user_id).unwrap();
            }
            let response = client
                .get(uri)
                .bearer_auth(&github_token)
                .header("user-agent", "zed")
                .send()
                .await
                .expect("failed to fetch github users");
            let users = response
                .json::<Vec<GitHubUser>>()
                .await
                .expect("failed to deserialize github user");
            zed_users.extend(users.iter().map(|user| (user.login.clone(), None)));

            if let Some(last_user) = users.last() {
                last_user_id = Some(last_user.id);
            } else {
                break;
            }
        }
    }

    let mut zed_user_ids = Vec::<UserId>::new();
    for (zed_user, email) in zed_users {
        if let Some(user) = db
            .get_user_by_github_login(&zed_user)
            .await
            .expect("failed to fetch user")
        {
            zed_user_ids.push(user.id);
        } else {
            zed_user_ids.push(
                db.create_user(&zed_user, email, true)
                    .await
                    .expect("failed to insert user"),
            );
        }
    }

    let zed_org_id = if let Some(org) = db
        .find_org_by_slug("zed")
        .await
        .expect("failed to fetch org")
    {
        org.id
    } else {
        db.create_org("Zed", "zed")
            .await
            .expect("failed to insert org")
    };

    let general_channel_id = if let Some(channel) = db
        .get_org_channels(zed_org_id)
        .await
        .expect("failed to fetch channels")
        .iter()
        .find(|c| c.name == "General")
    {
        channel.id
    } else {
        let channel_id = db
            .create_org_channel(zed_org_id, "General")
            .await
            .expect("failed to insert channel");

        let now = OffsetDateTime::now_utc();
        let max_seconds = Duration::days(100).as_seconds_f64();
        let mut timestamps = (0..1000)
            .map(|_| now - Duration::seconds_f64(rng.gen_range(0_f64..=max_seconds)))
            .collect::<Vec<_>>();
        timestamps.sort();
        for timestamp in timestamps {
            let sender_id = *zed_user_ids.choose(&mut rng).unwrap();
            let body = lipsum::lipsum_words(rng.gen_range(1..=50));
            db.create_channel_message(channel_id, sender_id, &body, timestamp, rng.gen())
                .await
                .expect("failed to insert message");
        }
        channel_id
    };

    for user_id in zed_user_ids {
        db.add_org_member(zed_org_id, user_id, true)
            .await
            .expect("failed to insert org membership");
        db.add_channel_member(general_channel_id, user_id, true)
            .await
            .expect("failed to insert channel membership");
    }
}
