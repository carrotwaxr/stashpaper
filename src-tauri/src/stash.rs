use crate::error::AppError;
use crate::settings::Settings;
use image::ImageFormat;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
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
    #[allow(dead_code)]
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
    variables: Value,
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

    // Follow redirects only within the same origin, so the ApiKey header never
    // goes to another host.
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt
            .previous()
            .last()
            .is_some_and(|prev| prev.origin() == attempt.url().origin());
        if !same_origin {
            attempt.stop()
        } else if attempt.previous().len() > 5 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    });

    Client::builder()
        .default_headers(headers)
        .redirect(redirects)
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Stash(e.to_string()))
}

pub fn client_for(settings: &Settings) -> Result<Client, AppError> {
    build_client(&settings.api_key)
}

pub async fn test_connection(url: &str, api_key: &str) -> Result<bool, AppError> {
    let client = build_client(api_key)?;

    let body = GraphQLRequest {
        query: "query { systemStatus { databaseSchema } }".into(),
        variables: json!({}),
    };

    let resp = client
        .post(format!("{}/graphql", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    Ok(resp.status().is_success())
}

/// The user's query filter, split into the two `findImages` arguments.
#[derive(Debug, Default)]
pub struct QueryFilter {
    filter: Map<String, Value>,
    image_filter: Map<String, Value>,
}

/// Parse the query filter JSON. This fails closed: anything that can't be applied
/// exactly as written is an error, never an empty filter, because an empty filter
/// rotates through the whole library. Only a blank filter means "no filter".
pub fn parse_query_filter(raw: &str) -> Result<QueryFilter, AppError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(QueryFilter::default());
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| AppError::Settings(format!("Query filter is not valid JSON: {}", e)))?;
    let Value::Object(top) = value else {
        return Err(AppError::Settings(
            "Query filter must be a JSON object with `filter` and/or `image_filter`".into(),
        ));
    };

    let mut parsed = QueryFilter::default();
    for (key, value) in top {
        let target = match key.as_str() {
            "filter" => &mut parsed.filter,
            "image_filter" => &mut parsed.image_filter,
            _ => {
                return Err(AppError::Settings(format!(
                    "Unknown key `{}` in query filter: use `filter` and/or `image_filter`",
                    key
                )))
            }
        };
        let Value::Object(map) = value else {
            return Err(AppError::Settings(format!(
                "`{}` in the query filter must be a JSON object",
                key
            )));
        };
        *target = map;
    }
    Ok(parsed)
}

/// Build the `findImages` variables from the user's query filter, merging in
/// pagination (per_page, page), the min_resolution filter and the random seed sort.
fn build_variables(
    settings: &Settings,
    per_page: usize,
    page: usize,
    random_seed: Option<u64>,
) -> Result<Value, AppError> {
    let QueryFilter {
        mut filter,
        mut image_filter,
    } = parse_query_filter(&settings.query_filter)?;

    // Inject min_resolution unless the user has their own resolution filter
    if let Some(resolution_filter) = settings.min_resolution.to_stash_filter() {
        image_filter
            .entry("resolution")
            .or_insert(resolution_filter);
    }

    filter.insert("per_page".into(), json!(per_page));
    filter.insert("page".into(), json!(page));

    // Inject the seeded random sort for no-repeat random rotation, replacing no
    // sort, "random" or an older "random_<seed>". A non-random sort (e.g.
    // "rating") is the user's choice and stays.
    if let Some(seed) = random_seed {
        let replace = match filter.get("sort").and_then(Value::as_str) {
            None => true,
            Some(s) => s == "random" || s.starts_with("random_"),
        };
        if replace {
            filter.insert("sort".into(), json!(format!("random_{}", seed)));
        }
    }

    Ok(json!({
        "filter": filter,
        "image_filter": image_filter,
    }))
}

fn auth_error(status: StatusCode) -> AppError {
    AppError::Stash(format!(
        "Stash rejected the API key (HTTP {})",
        status.as_u16()
    ))
}

/// Run `findImages`. GraphQL errors come back with their own message even on a
/// non-2xx status, since Stash reports a bad filter as HTTP 422 with a JSON body.
async fn find_images(
    client: &Client,
    settings: &Settings,
    variables: Value,
) -> Result<FindImagesResult, AppError> {
    let resp = client
        .post(format!(
            "{}/graphql",
            settings.stash_url.trim_end_matches('/')
        ))
        .json(&GraphQLRequest {
            query: FIND_IMAGES_QUERY.into(),
            variables,
        })
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(auth_error(status));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    match serde_json::from_str::<GraphQLResponse<FindImagesData>>(&body) {
        Ok(gql) => {
            if let Some(err) = gql.errors.and_then(|errors| errors.into_iter().next()) {
                return Err(AppError::Stash(err.message));
            }
            if !status.is_success() {
                return Err(AppError::Stash(format!("Stash returned HTTP {}", status)));
            }
            gql.data
                .map(|d| d.find_images)
                .ok_or_else(|| AppError::Stash("Stash returned no data".into()))
        }
        Err(_) if !status.is_success() => {
            Err(AppError::Stash(format!("Stash returned HTTP {}", status)))
        }
        Err(e) => Err(AppError::Stash(format!(
            "Unexpected response from Stash: {}",
            e
        ))),
    }
}

/// Count the images the filter matches, fetching no image rows.
pub async fn query_image_count(client: &Client, settings: &Settings) -> Result<usize, AppError> {
    let result = find_images(client, settings, build_variables(settings, 0, 1, None)?).await?;
    Ok(result.count)
}

/// Fetch the image at `page` (one image per page), along with the current count.
pub async fn fetch_image_at_page(
    client: &Client,
    settings: &Settings,
    page: usize,
    random_seed: Option<u64>,
) -> Result<(usize, Option<StashImage>), AppError> {
    let result = find_images(
        client,
        settings,
        build_variables(settings, 1, page, random_seed)?,
    )
    .await?;
    Ok((result.count, result.images.into_iter().next()))
}

/// Test a query with the given settings, returning the image count.
/// Uses all mutations (min_resolution, etc.) but no seed.
pub async fn test_query(settings: &Settings) -> Result<usize, AppError> {
    query_image_count(&client_for(settings)?, settings).await
}

/// Milliseconds since the epoch, for unique cache filenames.
pub fn timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Delete old wallpaper files from the cache directory, keeping `keep`: the file
/// the desktop points at now.
pub fn clean_wallpaper_cache(cache_dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // current_wallpaper.* is the name builds before v0.1.0 used
        let ours = name.starts_with("wallpaper_") || name.starts_with("current_wallpaper.");
        if ours && path != keep {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// What came back for one image URL.
#[derive(Debug)]
pub enum Download {
    Saved(PathBuf),
    /// Not something a desktop can show: a video clip, an error page, a missing
    /// file. The rotation moves on to another image.
    Unusable(String),
}

/// Download one image into the cache directory. `index` keeps filenames unique
/// within a batch.
pub async fn download_image(
    client: &Client,
    image_url: &str,
    cache_dir: &Path,
    index: usize,
) -> Result<Download, AppError> {
    let resp = client
        .get(image_url)
        .send()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(auth_error(status));
    }
    if !status.is_success() {
        return Ok(Download::Unusable(format!("HTTP {}", status)));
    }

    // Stash serves the file as-is, so an image clip arrives as a video. Checking
    // the header first skips it without downloading the body.
    if let Some(content_type) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        if !content_type.to_ascii_lowercase().starts_with("image/") {
            return Ok(Download::Unusable(format!(
                "not an image ({})",
                content_type
            )));
        }
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;

    // Name the file by what the bytes are, not by what the server claims
    let ext = match image::guess_format(&bytes) {
        Ok(ImageFormat::Jpeg) => "jpg",
        Ok(ImageFormat::Png) => "png",
        Ok(ImageFormat::WebP) => "webp",
        Ok(ImageFormat::Gif) => "gif",
        Ok(ImageFormat::Bmp) => "bmp",
        Ok(other) => {
            return Ok(Download::Unusable(format!(
                "unsupported image format ({:?})",
                other
            )))
        }
        Err(_) => return Ok(Download::Unusable("not a recognizable image".into())),
    };

    std::fs::create_dir_all(cache_dir)?;

    // Use a unique filename so GNOME/KDE detect the wallpaper changed
    // (they cache by path and may not notice the file content changed)
    let file_path = cache_dir.join(format!(
        "wallpaper_{}_{}.{}",
        timestamp_millis(),
        index,
        ext
    ));
    std::fs::write(&file_path, &bytes)?;

    Ok(Download::Saved(file_path))
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
        let vars = build_variables(&settings, 1, 5, None).unwrap();
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
            }"#
            .into(),
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 3, None).unwrap();
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
        let vars = build_variables(&settings, 1, 1, None).unwrap();
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
        let vars = build_variables(&settings, 1, 1, None).unwrap();
        assert_eq!(vars["image_filter"]["resolution"]["value"], "STANDARD_HD");
        assert_eq!(
            vars["image_filter"]["resolution"]["modifier"],
            "GREATER_THAN"
        );
    }

    #[test]
    fn test_build_variables_no_override_user_resolution() {
        let settings = Settings {
            stash_url: "http://localhost:9999".into(),
            api_key: "key".into(),
            query_filter:
                r#"{"image_filter": {"resolution": {"value": "FOUR_K", "modifier": "EQUALS"}}}"#
                    .into(),
            min_resolution: MinResolution::Hd720,
            ..Settings::default()
        };
        let vars = build_variables(&settings, 1, 1, None).unwrap();
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
        let vars = build_variables(&settings, 1, 1, Some(42)).unwrap();
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
        let vars = build_variables(&settings, 1, 1, Some(99)).unwrap();
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
        let vars = build_variables(&settings, 1, 1, Some(99)).unwrap();
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
        let vars = build_variables(&settings, 1, 1, Some(99)).unwrap();
        // User's non-random sort should be preserved
        assert_eq!(vars["filter"]["sort"], "rating");
    }

    fn settings_for(stash_url: &str, query_filter: &str) -> Settings {
        Settings {
            stash_url: stash_url.into(),
            api_key: "key".into(),
            query_filter: query_filter.into(),
            ..Settings::default()
        }
    }

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn test_parse_query_filter_blank_means_no_filter() {
        for raw in ["", "   ", "{}"] {
            let parsed = parse_query_filter(raw).unwrap();
            assert!(parsed.filter.is_empty() && parsed.image_filter.is_empty());
        }
    }

    #[test]
    fn test_parse_query_filter_keeps_both_parts() {
        let parsed = parse_query_filter(
            r#"{"filter": {"sort": "rating"}, "image_filter": {"rating100": {"value": 80, "modifier": "GREATER_THAN"}}}"#,
        )
        .unwrap();
        assert_eq!(parsed.filter["sort"], "rating");
        assert_eq!(parsed.image_filter["rating100"]["value"], 80);
    }

    #[test]
    fn test_parse_query_filter_rejects_what_it_cannot_apply() {
        let cases = [
            ("{not json", "not valid JSON"),
            (r#"["filter"]"#, "must be a JSON object"),
            (r#""image_filter""#, "must be a JSON object"),
            // a typo in the wrapper key
            (
                r#"{"imagefilter": {"tags": {}}}"#,
                "Unknown key `imagefilter`",
            ),
            // the inner image_filter pasted without its wrapper
            (
                r#"{"tags": {"value": ["1"], "modifier": "INCLUDES"}}"#,
                "Unknown key `tags`",
            ),
            (r#"{"filter": []}"#, "`filter` in the query filter must be"),
            (
                r#"{"image_filter": null}"#,
                "`image_filter` in the query filter must be",
            ),
        ];
        for (raw, expected) in cases {
            let err = parse_query_filter(raw).unwrap_err().to_string();
            assert!(err.contains(expected), "{raw}: got {err}");
        }
    }

    #[test]
    fn test_build_variables_fails_on_unusable_filter() {
        let settings = settings_for("http://localhost:9999", r#"{"imagefilter": {}}"#);
        assert!(build_variables(&settings, 1, 1, None).is_err());
    }

    #[test]
    fn test_clean_wallpaper_cache_keeps_current_and_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "wallpaper_1_0.jpg",
            "wallpaper_2_0.png",
            "wallpaper_composite_3.jpg",
            "current_wallpaper.jpg",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        clean_wallpaper_cache(dir.path(), &dir.path().join("wallpaper_2_0.png"));

        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["notes.txt", "wallpaper_2_0.png"]);
    }

    mod http {
        use super::*;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        async fn serve(server: &MockServer, at: &str, response: ResponseTemplate) {
            Mock::given(method("GET"))
                .and(path(at))
                .respond_with(response)
                .mount(server)
                .await;
        }

        async fn download(
            server: &MockServer,
            at: &str,
        ) -> (Result<Download, AppError>, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            let client = build_client("key").unwrap();
            let url = format!("{}{}", server.uri(), at);
            (download_image(&client, &url, dir.path(), 0).await, dir)
        }

        #[tokio::test]
        async fn saves_an_image_named_by_its_bytes() {
            let server = MockServer::start().await;
            // a PNG labelled as JPEG: the file extension follows the bytes
            serve(
                &server,
                "/img",
                ResponseTemplate::new(200).set_body_raw(png_bytes(), "image/jpeg"),
            )
            .await;
            let (result, _dir) = download(&server, "/img").await;
            let Download::Saved(path) = result.unwrap() else {
                panic!("expected Saved");
            };
            assert_eq!(path.extension().unwrap(), "png");
            assert_eq!(std::fs::read(&path).unwrap(), png_bytes());
        }

        #[tokio::test]
        async fn sends_the_api_key() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(header("ApiKey", "key"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(png_bytes(), "image/png"))
                .expect(1)
                .mount(&server)
                .await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(matches!(result.unwrap(), Download::Saved(_)));
        }

        #[tokio::test]
        async fn skips_what_a_desktop_cannot_show() {
            let server = MockServer::start().await;
            serve(
                &server,
                "/clip",
                ResponseTemplate::new(200)
                    .set_body_raw(b"\x00\x00\x00\x18ftypmp42".to_vec(), "video/mp4"),
            )
            .await;
            serve(
                &server,
                "/page",
                ResponseTemplate::new(200)
                    .set_body_raw(b"<html>login</html>".to_vec(), "text/html"),
            )
            .await;
            serve(
                &server,
                "/lie",
                ResponseTemplate::new(200)
                    .set_body_raw(b"<html>oops</html>".to_vec(), "image/jpeg"),
            )
            .await;
            serve(
                &server,
                "/missing",
                ResponseTemplate::new(404).set_body_string("not found"),
            )
            .await;
            serve(
                &server,
                "/broken",
                ResponseTemplate::new(500).set_body_string("open failed"),
            )
            .await;

            for (at, reason) in [
                ("/clip", "not an image (video/mp4)"),
                ("/page", "not an image (text/html)"),
                ("/lie", "not a recognizable image"),
                ("/missing", "HTTP 404"),
                ("/broken", "HTTP 500"),
            ] {
                let (result, dir) = download(&server, at).await;
                match result.unwrap() {
                    Download::Unusable(got) => assert!(got.contains(reason), "{at}: {got}"),
                    Download::Saved(p) => panic!("{at}: saved {}", p.display()),
                }
                // nothing unusable lands in the cache
                assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{at}");
            }
        }

        #[tokio::test]
        async fn a_rejected_key_fails_the_rotation() {
            let server = MockServer::start().await;
            serve(&server, "/img", ResponseTemplate::new(401)).await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("rejected the API key"));
        }

        #[tokio::test]
        async fn does_not_follow_a_redirect_to_another_host() {
            let other = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(png_bytes(), "image/png"))
                .expect(0)
                .mount(&other)
                .await;
            let server = MockServer::start().await;
            serve(
                &server,
                "/img",
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/elsewhere", other.uri()).as_str()),
            )
            .await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(matches!(result.unwrap(), Download::Unusable(r) if r.contains("302")));
        }

        #[tokio::test]
        async fn follows_a_redirect_on_the_same_host() {
            let server = MockServer::start().await;
            serve(
                &server,
                "/img",
                ResponseTemplate::new(302).insert_header("Location", "/real"),
            )
            .await;
            serve(
                &server,
                "/real",
                ResponseTemplate::new(200).set_body_raw(png_bytes(), "image/png"),
            )
            .await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(matches!(result.unwrap(), Download::Saved(_)));
        }

        #[tokio::test]
        async fn find_images_reports_graphql_errors_even_on_422() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                    "errors": [{"message": "unknown field tagz"}], "data": null
                })))
                .mount(&server)
                .await;
            let settings = settings_for(&server.uri(), "{}");
            let client = client_for(&settings).unwrap();
            let err = query_image_count(&client, &settings).await.unwrap_err();
            assert!(err.to_string().contains("unknown field tagz"), "{err}");
        }

        #[tokio::test]
        async fn find_images_names_a_rejected_key() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
                .mount(&server)
                .await;
            let settings = settings_for(&server.uri(), "{}");
            let client = client_for(&settings).unwrap();
            let err = query_image_count(&client, &settings).await.unwrap_err();
            assert!(err.to_string().contains("rejected the API key"), "{err}");
        }

        #[tokio::test]
        async fn find_images_names_a_non_json_error() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
                .mount(&server)
                .await;
            let settings = settings_for(&server.uri(), "{}");
            let client = client_for(&settings).unwrap();
            let err = query_image_count(&client, &settings).await.unwrap_err();
            assert!(err.to_string().contains("HTTP 502"), "{err}");
        }

        #[tokio::test]
        async fn fetch_returns_the_count_with_the_image() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": {"findImages": {"count": 7, "images": [
                        {"id": "3", "paths": {"image": "http://x/image/3"}}
                    ]}}
                })))
                .mount(&server)
                .await;
            let settings = settings_for(&server.uri(), "{}");
            let client = client_for(&settings).unwrap();
            let (count, image) = fetch_image_at_page(&client, &settings, 2, Some(5))
                .await
                .unwrap();
            assert_eq!(count, 7);
            assert_eq!(image.unwrap().paths.image.unwrap(), "http://x/image/3");
        }
    }
}
