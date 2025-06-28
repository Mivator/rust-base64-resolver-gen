use actix_web::{web, App, HttpResponse, HttpServer, Responder, Result, Error};
use actix_multipart::Multipart;
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::sync::Mutex;
use lru::LruCache;use std::num::NonZeroUsize;
use uuid::Uuid;

const HOSTNAME: &str = "0.0.0.0"; // Update with your desired hostname
const PORT: u16 = 3555; // Update with your desired port
const CACHE_SIZE: usize = 10_000; // update


struct AppState {
    images: Mutex<LruCache<String, Vec<u8>>>,
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let app_state = web::Data::new(AppState {
        images: Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap())),
    });

    // log the running hostname and port
    println!("Running on {}:{}", HOSTNAME, PORT);

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .route("/image", web::post().to(post_image))
            .route("/image/multipart", web::post().to(post_image_multipart))
            .route("/image/{id}", web::get().to(get_image))
    })
    .bind((HOSTNAME, PORT))?
    .run()
    .await
}
