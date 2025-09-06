use actix_web::{web, App, HttpResponse, HttpServer, Responder, Result, Error};
use actix_multipart::Multipart;
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::sync::Mutex;
use lru::LruCache;
use std::num::NonZeroUsize;
use uuid::Uuid;
use deadpool_postgres::{Pool, Runtime};
use tokio_postgres::NoTls;
use config::{Config as ConfigFile, File, Environment};

const CACHE_SIZE: usize = 10_000; // update

#[derive(Debug, Deserialize)]
struct Settings {
    server: ServerSettings,
    database: DatabaseSettings,
}

#[derive(Debug, Deserialize)]
struct ServerSettings {
    hostname: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct DatabaseSettings {
    url: String,
}


struct AppState {
    images: Mutex<LruCache<String, Vec<u8>>>,
    db_pool: Pool,
}

#[derive(Deserialize)]
struct Base64Payload {
    base64: String,
}

async fn post_image(
    payload: web::Either<web::Json<Base64Payload>, web::Form<Base64Payload>>,
    data: web::Data<AppState>,
) -> Result<impl Responder, Error> {
    let base64_str = match payload {
        web::Either::Left(json) => json.base64.clone(),
        web::Either::Right(form) => form.base64.clone(),
    };

    println!("Received Base64 data ({} chars)", base64_str.len());

    let decoded_bytes = match STANDARD.decode(&base64_str) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(HttpResponse::BadRequest().body("Invalid Base64 encoding")),
    };

    let id = Uuid::new_v4().to_string();
    let mut cache = data.images.lock().unwrap();
    cache.put(id.clone(), decoded_bytes);

    let path = format!("/image/{}", id);
    Ok(HttpResponse::Ok().json(json!({ "urlPath": path })))
}

async fn post_image_multipart(mut payload: Multipart, data: web::Data<AppState>) -> Result<impl Responder, Error> {
    let mut base64_str = String::new();

    while let Some(mut field) = payload.try_next().await? {
        if field.name() == "base64" {
            while let Some(chunk) = field.next().await {
                let chunk = chunk?;
                base64_str.push_str(&String::from_utf8_lossy(&chunk));
            }
        }
    }

    if base64_str.is_empty() {
        return Ok(HttpResponse::BadRequest().body("Missing 'base64' field"));
    }

    println!("Received Base64 data ({} chars) via FormData", base64_str.len());

    let decoded_bytes = match STANDARD.decode(&base64_str) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(HttpResponse::BadRequest().body("Invalid Base64 encoding")),
    };

    let id = Uuid::new_v4().to_string();
    let mut cache = data.images.lock().unwrap();
    cache.put(id.clone(), decoded_bytes);

    let path = format!("/image/{}", id);
    Ok(HttpResponse::Ok().json(json!({ "urlPath": path })))
}

async fn get_image(data: web::Data<AppState>, id: web::Path<String>) -> Result<impl Responder> {
    let id = id.into_inner();

    let mut cache = data.images.lock().unwrap();
    match cache.get(&id) {
        Some(image_data) => Ok(HttpResponse::Ok()
            .content_type("image/png")
            .body(image_data.clone())),
        None => Ok(HttpResponse::NotFound().body("Image not found")),
    }
}

async fn get_image_by_md5(data: web::Data<AppState>, hash: web::Path<String>) -> Result<impl Responder, Error> {
    let hash = hash.into_inner();

    // Validate MD5 hash format (32 hex characters)
    if hash.len() != 32 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(HttpResponse::BadRequest().body("Invalid MD5 hash format"));
    }

    let client = data.db_pool.get().await.map_err(|e| {
        eprintln!("Database connection error: {}", e);
        actix_web::error::ErrorInternalServerError("Database connection failed")
    })?;

    // Query the iconAndAvatars table for the image data by hash
    let row = client
        .query_opt(
            "SELECT \"imageData\" FROM \"iconAndAvatars\" WHERE hash = $1",
            &[&hash]
        )
        .await
        .map_err(|e| {
            eprintln!("Database query error (iconAndAvatars): {}", e);
            actix_web::error::ErrorInternalServerError("Database query failed")
        })?;

    match row {
        Some(row) => {
            let base64_str: String = row.get("imageData");
            match STANDARD.decode(&base64_str) {
                Ok(image_data) => Ok(HttpResponse::Ok()
                    .content_type("image/png")
                    .body(image_data)),
                Err(e) => {
                    eprintln!("Failed to decode base64 from iconAndAvatars: {}", e);
                    Ok(HttpResponse::InternalServerError().body("Failed to decode image data"))
                }
            }
        }
        None => Ok(HttpResponse::NotFound().body("Image not found")),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load configuration
    let settings = ConfigFile::builder()
        .add_source(File::with_name("config").required(false))
        .add_source(Environment::with_prefix("APP"))
        .build()
        .map_err(|e| {
            eprintln!("Failed to load configuration: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Configuration loading failed")
        })?;

    let settings: Settings = settings.try_deserialize().map_err(|e| {
        eprintln!("Failed to deserialize configuration: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, "Configuration deserialization failed")
    })?;

    println!("Configuration loaded: {:?}", settings);

    // Database configuration using URL
    let mut cfg = deadpool_postgres::Config::new();
    cfg.url = Some(settings.database.url.clone());

    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|e| {
            eprintln!("Failed to create database pool: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Database pool creation failed")
        })?;

    // Initialize database schema
    let client = pool.get().await.map_err(|e| {
        eprintln!("Failed to get database client: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, "Database client creation failed")
    })?;

    // Create the iconAndAvatars table if it doesn't exist (matching Prisma schema)
    client.execute(
        "CREATE TABLE IF NOT EXISTS \"iconAndAvatars\" (
            \"snowflakeTargetId\" TEXT NOT NULL,
            hash TEXT NOT NULL,
            \"imageData\" TEXT NOT NULL,
            \"metaNameData\" TEXT NOT NULL,
            \"savedAt\" TIMESTAMP DEFAULT NOW(),
            PRIMARY KEY (\"snowflakeTargetId\", hash)
        )",
        &[]
    ).await.map_err(|e| {
        eprintln!("Failed to create iconAndAvatars table: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, "Database table creation failed")
    })?;

    let app_state = web::Data::new(AppState {
        images: Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
        db_pool: pool,
    });

    // log the running hostname and port
    println!("Running on {}:{}", settings.server.hostname, settings.server.port);

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .route("/image", web::post().to(post_image))
            .route("/image/multipart", web::post().to(post_image_multipart))
            .route("/image/{id}", web::get().to(get_image))
            .route("/md5/{hash}", web::get().to(get_image_by_md5))
    })
    .bind((settings.server.hostname.as_str(), settings.server.port))?
    .run()
    .await
}
