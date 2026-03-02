use crate::error::AppError;
use crate::settings::Settings;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FindImagesData {
    find_images: FindImagesResult,
}

#[derive(Debug, Deserialize)]
pub struct FindImagesResult {
    pub count: usize,
    pub images: Vec<StashImage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StashImage {
    pub id: String,
    pub paths: ImagePaths,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImagePaths {
    pub image: Option<String>,
}

#[derive(Debug, Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

const FIND_IMAGES_QUERY: &str = r#"
query FindImages($filter: FindFilterType, $image_filter: ImageFilterType) {
  findImages(filter: $filter, image_filter: $image_filter) {
    count
    images {
      id
      paths {
        image
      }
    }
  }
}
"#;

fn build_client(api_key: &str) -> Result<Client, AppError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "ApiKey",
        api_key
            .trim()
            .parse()
            .map_err(|e: reqwest::header::InvalidHeaderValue| AppError::Stash(e.to_string()))?,
    );

    Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Stash(e.to_string()))
}

fn client_for(settings: &Settings) -> Result<Client, AppError> {
    build_client(&settings.api_key)
}

pub async fn test_connection(url: &str, api_key: &str) -> Result<bool, AppError> {
    let client = build_client(api_key)?;

    let body = GraphQLRequest {
        query: "query { systemStatus { databaseSchema } }".into(),
        variables: serde_json::json!({}),
    };

    let resp = client
        .post(format!("{}/graphql", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    Ok(resp.status().is_success())
}

pub async fn query_image_count(settings: &Settings) -> Result<usize, AppError> {
    let client = client_for(settings)?;
    let image_filter: serde_json::Value =
        serde_json::from_str(&settings.image_filter).unwrap_or(serde_json::json!({}));

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: serde_json::json!({
            "filter": { "per_page": 1, "page": 1 },
            "image_filter": image_filter,
        }),
    };

    let resp = client
        .post(format!(
            "{}/graphql",
            settings.stash_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let gql: GraphQLResponse<FindImagesData> =
        resp.json().await.map_err(|e| AppError::Stash(e.to_string()))?;

    if let Some(errors) = gql.errors {
        if let Some(err) = errors.first() {
            return Err(AppError::Stash(err.message.clone()));
        }
    }

    Ok(gql
        .data
        .map(|d| d.find_images.count)
        .unwrap_or(0))
}

pub async fn fetch_image_at_page(
    settings: &Settings,
    page: usize,
) -> Result<Option<StashImage>, AppError> {
    let client = client_for(settings)?;
    let image_filter: serde_json::Value =
        serde_json::from_str(&settings.image_filter).unwrap_or(serde_json::json!({}));

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: serde_json::json!({
            "filter": { "per_page": 1, "page": page },
            "image_filter": image_filter,
        }),
    };

    let resp = client
        .post(format!(
            "{}/graphql",
            settings.stash_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let gql: GraphQLResponse<FindImagesData> =
        resp.json().await.map_err(|e| AppError::Stash(e.to_string()))?;

    if let Some(errors) = gql.errors {
        if let Some(err) = errors.first() {
            return Err(AppError::Stash(err.message.clone()));
        }
    }

    Ok(gql
        .data
        .and_then(|d| d.find_images.images.into_iter().next()))
}

pub async fn download_image(
    settings: &Settings,
    image_url: &str,
    cache_dir: &Path,
) -> Result<PathBuf, AppError> {
    let client = client_for(settings)?;

    let resp = client
        .get(image_url)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg");

    let ext = match content_type {
        t if t.contains("png") => "png",
        t if t.contains("webp") => "webp",
        t if t.contains("gif") => "gif",
        _ => "jpg",
    };

    std::fs::create_dir_all(cache_dir)?;
    let file_path = cache_dir.join(format!("current_wallpaper.{}", ext));

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;
    std::fs::write(&file_path, &bytes)?;

    Ok(file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphql_request_serialization() {
        let body = GraphQLRequest {
            query: FIND_IMAGES_QUERY.into(),
            variables: serde_json::json!({
                "filter": { "per_page": 1, "page": 1 },
                "image_filter": {},
            }),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("findImages"));
        assert!(json.contains("per_page"));
    }

    #[test]
    fn test_find_images_response_parsing() {
        let json = r#"{
            "data": {
                "findImages": {
                    "count": 42,
                    "images": [{
                        "id": "123",
                        "paths": {
                            "image": "http://localhost:9999/image/123/image"
                        }
                    }]
                }
            }
        }"#;

        let resp: GraphQLResponse<FindImagesData> = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.find_images.count, 42);
        assert_eq!(data.find_images.images.len(), 1);
        assert_eq!(data.find_images.images[0].id, "123");
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "data": null,
            "errors": [{"message": "Something went wrong"}]
        }"#;

        let resp: GraphQLResponse<FindImagesData> = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_none());
        assert_eq!(resp.errors.unwrap()[0].message, "Something went wrong");
    }

    #[test]
    fn test_image_filter_parsing_empty() {
        let filter: serde_json::Value =
            serde_json::from_str("{}").unwrap_or(serde_json::json!({}));
        assert!(filter.is_object());
    }

    #[test]
    fn test_image_filter_parsing_with_tags() {
        let filter_str = r#"{"tags":{"value":["wallpaper"],"modifier":"INCLUDES_ALL"}}"#;
        let filter: serde_json::Value = serde_json::from_str(filter_str).unwrap();
        assert!(filter["tags"]["value"].is_array());
    }
}
