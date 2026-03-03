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

/// Parse the combined query_filter JSON into separate filter and image_filter values,
/// merging pagination (per_page, page) into the user's filter.
/// Optionally injects min_resolution and random seed sort.
fn build_variables(
    settings: &Settings,
    per_page: usize,
    page: usize,
    random_seed: Option<u64>,
) -> serde_json::Value {
    let parsed: serde_json::Value =
        serde_json::from_str(&settings.query_filter).unwrap_or(serde_json::json!({}));

    let mut image_filter = parsed
        .get("image_filter")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Inject min_resolution if set and user hasn't specified their own resolution filter
    if let Some(resolution_filter) = settings.min_resolution.to_stash_filter() {
        if let Some(obj) = image_filter.as_object_mut() {
            if !obj.contains_key("resolution") {
                obj.insert("resolution".into(), resolution_filter);
            }
        }
    }

    // Start with user's filter, then override pagination
    let mut filter = match parsed.get("filter") {
        Some(f) => f.clone(),
        None => serde_json::json!({}),
    };
    if let Some(obj) = filter.as_object_mut() {
        obj.insert("per_page".into(), serde_json::json!(per_page));
        obj.insert("page".into(), serde_json::json!(page));

        // Inject random seed sort for no-repeat random rotation
        if let Some(seed) = random_seed {
            let seeded_sort = format!("random_{}", seed);
            match obj.get("sort").and_then(|v| v.as_str()) {
                // No sort specified — inject seeded random
                None => {
                    obj.insert("sort".into(), serde_json::json!(seeded_sort));
                }
                // User has "random" or already-seeded "random_<digits>" — replace with our seed
                Some(s) if s == "random" || s.starts_with("random_") => {
                    obj.insert("sort".into(), serde_json::json!(seeded_sort));
                }
                // User has a non-random sort (e.g. "rating", "date") — respect it
                Some(_) => {}
            }
        }
    }

    serde_json::json!({
        "filter": filter,
        "image_filter": image_filter,
    })
}

pub async fn query_image_count(settings: &Settings) -> Result<usize, AppError> {
    let client = client_for(settings)?;

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: build_variables(settings, 1, 1, None),
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
    random_seed: Option<u64>,
) -> Result<Option<StashImage>, AppError> {
    let client = client_for(settings)?;

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: build_variables(settings, 1, page, random_seed),
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

/// Test a query with the given settings, returning the image count.
/// Uses all mutations (min_resolution, etc.) but no seed.
pub async fn test_query(settings: &Settings) -> Result<usize, AppError> {
    let client = client_for(settings)?;

    let body = GraphQLRequest {
        query: FIND_IMAGES_QUERY.into(),
        variables: build_variables(settings, 0, 1, None),
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

    // Use a unique filename so GNOME/KDE detect the wallpaper changed
    // (they cache by path and may not notice the file content changed)
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let file_path = cache_dir.join(format!("wallpaper_{}.{}", timestamp, ext));

    // Clean up old wallpaper files
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("wallpaper_") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

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
    use crate::settings::MinResolution;

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
    fn test_build_variables_empty_filter() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: "{}".into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 5, None);
        assert_eq!(vars["filter"]["per_page"], 1);
        assert_eq!(vars["filter"]["page"], 5);
        assert!(vars["image_filter"].is_object());
    }

    #[test]
    fn test_build_variables_with_user_filter() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{
                "filter": { "sort": "random", "direction": "DESC" },
                "image_filter": {
                    "orientation": { "value": "LANDSCAPE" },
                    "rating100": { "value": 90, "modifier": "GREATER_THAN" }
                }
            }"#.into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 3, None);
        // User's sort/direction preserved (no seed passed)
        assert_eq!(vars["filter"]["sort"], "random");
        assert_eq!(vars["filter"]["direction"], "DESC");
        // Pagination merged in
        assert_eq!(vars["filter"]["per_page"], 1);
        assert_eq!(vars["filter"]["page"], 3);
        // Image filter preserved
        assert_eq!(vars["image_filter"]["orientation"]["value"], "LANDSCAPE");
        assert_eq!(vars["image_filter"]["rating100"]["value"], 90);
    }

    #[test]
    fn test_build_variables_image_filter_only() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"image_filter": {"tags": {"value": ["wallpaper"], "modifier": "INCLUDES_ALL"}}}"#.into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, None);
        assert!(vars["image_filter"]["tags"]["value"].is_array());
        assert_eq!(vars["filter"]["per_page"], 1);
    }

    #[test]
    fn test_build_variables_injects_min_resolution() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"image_filter": {}}"#.into(),
            min_resolution: MinResolution::FullHd1080,
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, None);
        assert_eq!(vars["image_filter"]["resolution"]["value"], "STANDARD_HD");
        assert_eq!(vars["image_filter"]["resolution"]["modifier"], "GREATER_THAN");
    }

    #[test]
    fn test_build_variables_no_override_user_resolution() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"image_filter": {"resolution": {"value": "FOUR_K", "modifier": "EQUALS"}}}"#.into(),
            min_resolution: MinResolution::Hd720,
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, None);
        // User's resolution filter preserved, not overridden by min_resolution
        assert_eq!(vars["image_filter"]["resolution"]["value"], "FOUR_K");
        assert_eq!(vars["image_filter"]["resolution"]["modifier"], "EQUALS");
    }

    #[test]
    fn test_build_variables_with_random_seed() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: "{}".into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, Some(42));
        assert_eq!(vars["filter"]["sort"], "random_42");
    }

    #[test]
    fn test_build_variables_seed_replaces_random_sort() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"filter": {"sort": "random"}}"#.into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, Some(99));
        assert_eq!(vars["filter"]["sort"], "random_99");
    }

    #[test]
    fn test_build_variables_seed_replaces_existing_seeded_sort() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"filter": {"sort": "random_12345"}}"#.into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, Some(99));
        assert_eq!(vars["filter"]["sort"], "random_99");
    }

    #[test]
    fn test_build_variables_seed_respects_non_random_sort() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter: r#"{"filter": {"sort": "rating"}}"#.into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, Some(99));
        // User's non-random sort should be preserved
        assert_eq!(vars["filter"]["sort"], "rating");
    }
}
