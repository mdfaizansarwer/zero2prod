//! tests/health_check.rs

use once_cell::sync::Lazy;
use secrecy::ExposeSecret;
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::net::TcpListener;
use uuid::Uuid;
use zero2prod::{
    configuration::{get_configuration, DatabaseSettings},
    startup::run,
    telemetry::{get_subscriber, init_subscriber},
};

static TRACING: Lazy<()> = Lazy::new(|| {
    let default_filter_name = "info".to_string();
    let subscriber_name = "test".to_string();

    if std::env::var("TEST_LOG").is_ok() {
        let subscriber = get_subscriber(subscriber_name, default_filter_name, std::io::stdout);
        init_subscriber(subscriber);
    } else {
        let subscriber = get_subscriber(subscriber_name, default_filter_name, std::io::sink);
        init_subscriber(subscriber);
    };
});

pub struct TestApp {
    pub address: String,
    pub db_pool: PgPool,
}

async fn spawn_app() -> TestApp {
    Lazy::force(&TRACING);

    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();
    let address = format!("http://127.0.0.1:{}", port);

    let mut configuration = get_configuration().expect("Failed to get configuration");
    configuration.database.database_name = Uuid::new_v4().to_string();
    let db_pool = configure_database(&configuration.database).await;

    let server = run(listener, db_pool.clone()).expect("Failed to create address");
    let _ = tokio::spawn(server);
    TestApp { address, db_pool }
}

pub async fn configure_database(config: &DatabaseSettings) -> PgPool {
    // Create the database
    let mut connection =
        PgConnection::connect(&config.connection_string_without_db().expose_secret())
            .await
            .expect("Failed to connect with Postgres");

    connection
        .execute(&*format!(
            r#"create database "{}";"#,
            config.database_name.as_str()
        ))
        .await
        .expect("Failed to connect with database");

    // Migrate all the changes to this database
    let connection_pool = PgPool::connect(&config.connection_string().expose_secret())
        .await
        .expect("Failed to connection with postgres");
    sqlx::migrate!("./migrations")
        .run(&connection_pool)
        .await
        .expect("Failed to run the migration");

    connection_pool
}

#[tokio::test]
async fn health_check_works() {
    // Arrange
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    // Act
    let response = client
        .get(&format!("{}/health_check", &app.address))
        .send()
        .await
        .expect("Failed to execute request");

    // Assert
    assert!(response.status().is_success());
    assert_eq!(Some(0), response.content_length());
}

#[tokio::test]
async fn subscribe_returns_200_for_valid_form_data() {
    // Arrange
    let app = spawn_app().await;
    let client = reqwest::Client::new();
    let body = "name=faizan&email=faizan%40test.com";

    // Act
    let response = client
        .post(&format!("{}/subscriptions", &app.address))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .expect("Failed to execute request");

    // Assert
    assert_eq!(200, response.status().as_u16());

    let saved = sqlx::query!("SELECT name, email from subscriptions",)
        .fetch_one(&app.db_pool)
        .await
        .expect("Failed to fetch saved subscription");
    assert_eq!(saved.email, "faizan@test.com");
    assert_eq!(saved.name, "faizan");
}

#[tokio::test]
async fn subscribe_returns_400_when_fields_are_present_but_invalid() {
    // Arrange
    let app = spawn_app().await;
    let client = reqwest::Client::new();
    let test_cases = vec![
        ("name=&email=faizan%40gmail.com", "empty name"),
        ("name=Faizan&email=", "empty email"),
        ("name=Faizan&email=faizan-wrong-email", "empty name"),
    ];

    for (body, description) in test_cases {
        // Act
        let response = client
            .post(&format!("{}/subscriptions", &app.address))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .expect("Failed to execute request");

        // Assert
        assert_eq!(
            400,
            response.status().as_u16(),
            "The api did not return a 400 OK when the payload was {}",
            description
        );
    }
}

#[tokio::test]
async fn subscribe_returns_400_when_data_is_missing() {
    // Arrange
    let app = spawn_app().await;
    let client = reqwest::Client::new();
    let test_cases = vec![
        ("email=faian%40gmail.com", "Missing the name"),
        ("name=faian", "Missing the email"),
        ("", "Missing name and email"),
    ];

    for (invalid_data, error_message) in test_cases {
        // Act
        let response = client
            .post(&format!("{}/subscriptions", &app.address))
            .body(invalid_data)
            .send()
            .await
            .expect("Failed to execute  request");

        // Assert
        assert_eq!(
            400,
            response.status().as_u16(),
            "The api did not fail with 400 Bad Request when the payload was {}",
            error_message
        );
    }
}
