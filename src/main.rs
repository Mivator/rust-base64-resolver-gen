use actix_web::{web, App, HttpResponse, HttpServer, Responder, Result, Error};
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use mime::IMAGE_PNG;
use actix_web::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use std::sync::Mutex;
use std::collections::HashMap;
use uuid::Uuid;

struct AppState {
    images: Mutex<HashMap<String, Vec<u8>>>,
}

async fn post_image(
    mut payload: web::Payload,
    data: web::Data<AppState>,
) -> Result<impl Responder, Error> {
    let mut body = web::BytesMut::new();
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        body.extend_from_slice(&chunk);
    }

    let json: Value = serde_json::from_slice(&body)
        .map_err(|_| actix_web::error::ErrorBadRequest("Invalid JSON"))?;

    let base64_str = json["base64"].as_str()
        .ok_or_else(|| actix_web::error::ErrorBadRequest("Missing 'base64' field"))?;

    println!("Received POST request with base64 data length: {}", base64_str.len());

    let decoded_bytes = match STANDARD.decode(base64_str) {
        Ok(bytes) => bytes,
        Err(_) => {
            println!("Invalid Base64 encoding");
            return Ok(HttpResponse::BadRequest().body("Invalid Base64 encoding"));
        }
    };

    println!("Successfully decoded {} bytes", decoded_bytes.len());

    let id = Uuid::new_v4().to_string();
    data.images.lock().unwrap().insert(id.clone(), decoded_bytes);

    let path = format!("/image/{}", id);
    Ok(HttpResponse::Ok().json(json!({
        "urlPath": path,
    })))
}

async fn get_image(
    data: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<impl Responder> {
    let id = id.into_inner();
    println!("Received GET request for image ID: {}", id);

    let images = data.images.lock().unwrap();
    match images.get(&id) {
        Some(image_data) => {
            Ok(HttpResponse::Ok()
                .insert_header((CONTENT_TYPE, IMAGE_PNG.to_string()))
                .insert_header((CACHE_CONTROL, "public, max-age=86400"))
                .body(image_data.clone()))
        },
        None => Ok(HttpResponse::NotFound().body("Image not found")),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("Starting server on localhost:8080");

    let app_state = web::Data::new(AppState {
        images: Mutex::new(HashMap::new()),
    });

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .route("/image", web::post().to(post_image))
            .route("/image/{id}", web::get().to(get_image))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
