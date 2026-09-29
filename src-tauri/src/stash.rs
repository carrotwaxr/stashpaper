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

/// Whether a redirect keeps the ApiKey header on the same server: the same host
/// and port, or the same host upgraded from http to https (common behind a
/// reverse proxy). Anything else could hand the key to another service.
fn redirect_allowed(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    if from.host_str() != to.host_str() {
        return false;
    }
    let same_port = from.port_or_known_default() == to.port_or_known_default();
    let upgrade = from.scheme() == "http" && to.scheme() == "https" && to.port().is_none();
    (same_port && from.scheme() == to.scheme()) || upgrade
}

fn build_client(api_key: &str) -> Result<Client, AppError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "ApiKey",
        api_key
            .trim()
            .parse()
            .map_err(|e: reqwest::header::InvalidHeaderValue| AppError::Stash(e.to_string()))?,
    );

    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        let allowed = attempt
            .previous()
            .last()
            .is_some_and(|prev| redirect_allowed(prev, attempt.url()));
        if !allowed {
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
    // Trim a byte order mark as whitespace, as the settings window's JS trim() does
    let raw = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if raw.is_empty() {
        return Ok(QueryFilter::default());
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| AppError::Filter(format!("not valid JSON ({})", e)))?;
    let Value::Object(top) = value else {
        return Err(AppError::Filter(
            "must be a JSON object with filter and/or image_filter".into(),
        ));
    };

    let mut parsed = QueryFilter::default();
    for (key, value) in top {
        let target = match key.as_str() {
            "filter" => &mut parsed.filter,
            "image_filter" => &mut parsed.image_filter,
            _ => {
                return Err(AppError::Filter(format!(
                    "unknown key \"{}\", use filter and/or image_filter",
                    key
                )))
            }
        };
        let Value::Object(map) = value else {
            return Err(AppError::Filter(format!(
                "\"{}\" must be a JSON object",
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
    if status == StatusCode::FORBIDDEN {
        AppError::Stash("access denied (HTTP 403), check the API key in Settings".into())
    } else {
        AppError::Stash(format!(
            "the API key was rejected (HTTP {}), check it in Settings",
            status.as_u16()
        ))
    }
}

/// Run `findImages`. GraphQL errors come back with their own message even on a
/// non-2xx status, since Stash reports a bad filter as HTTP 422 with a JSON body.
async fn find_images(
    client: &Client,
    settings: &Settings,
    variables: Value,
) -> Result<FindImagesResult, AppError> {
    let url = format!("{}/graphql", settings.stash_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
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
    // A redirect turns the POST into a GET (except for 307/308), so a failure
    // after one is best explained as "use the address it redirects to"
    let redirected_to = (resp.url().as_str() != url).then(|| resp.url().clone());
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;
    match (parse_find_images(status, &body), redirected_to) {
        (Err(_), Some(to)) => Err(AppError::Stash(format!(
            "the server redirected to {}, use {} as the Server URL",
            to,
            to.origin().ascii_serialization()
        ))),
        (result, _) => result,
    }
}

fn parse_find_images(status: StatusCode, body: &str) -> Result<FindImagesResult, AppError> {
    match serde_json::from_str::<GraphQLResponse<FindImagesData>>(body) {
        Ok(gql) => {
            if let Some(err) = gql.errors.and_then(|errors| errors.into_iter().next()) {
                return Err(AppError::Stash(err.message));
            }
            if !status.is_success() {
                return Err(AppError::Stash(format!(
                    "the server returned HTTP {}",
                    status
                )));
            }
            gql.data
                .map(|d| d.find_images)
                .ok_or_else(|| AppError::Stash("the server returned no data".into()))
        }
        Err(_) if !status.is_success() => Err(AppError::Stash(format!(
            "the server returned HTTP {}",
            status
        ))),
        Err(e) => Err(AppError::Stash(format!("unexpected response ({})", e))),
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

/// Delete old wallpaper files from the cache directory, except those in `keep`:
/// the files the desktop may point at now.
pub fn clean_wallpaper_cache(cache_dir: &Path, keep: &[&Path]) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // current_wallpaper.* is the name builds before v0.1.0 used
        let ours = name.starts_with("wallpaper_") || name.starts_with("current_wallpaper.");
        if ours && !keep.contains(&path.as_path()) {
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
        return Ok(Download::Unusable(format!(
            "Stash returned HTTP {} for the file",
            status
        )));
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
                "the file is {}, not an image",
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
                "{:?} images aren't supported",
                other
            )))
        }
        Err(_) => {
            return Ok(Download::Unusable(
                "the file isn't a recognizable image".into(),
            ))
        }
    };

    // A good header can front a damaged or cut-short file, so decode it
    let check = bytes.clone();
    let decoded = tokio::task::spawn_blocking(move || image::load_from_memory(&check).map(|_| ()))
        .await
        .map_err(|e| AppError::Stash(e.to_string()))?;
    match decoded {
        Ok(()) => {}
        Err(image::ImageError::Limits(_)) => {
            return Ok(Download::Unusable("the image is too large to use".into()))
        }
        Err(_) => {
            return Ok(Download::Unusable(
                "the file is damaged or incomplete".into(),
            ))
        }
    }

    tokio::fs::create_dir_all(cache_dir).await?;

    // Use a unique filename so GNOME/KDE detect the wallpaper changed
    // (they cache by path and may not notice the file content changed)
    let file_path = cache_dir.join(format!(
        "wallpaper_{}_{}.{}",
        timestamp_millis(),
        index,
        ext
    ));
    tokio::fs::write(&file_path, &bytes).await?;

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
                "unknown key \"imagefilter\"",
            ),
            // the inner image_filter pasted without its wrapper
            (
                r#"{"tags": {"value": ["1"], "modifier": "INCLUDES"}}"#,
                "unknown key \"tags\"",
            ),
            (r#"{"filter": []}"#, "\"filter\" must be a JSON object"),
            (
                r#"{"image_filter": null}"#,
                "\"image_filter\" must be a JSON object",
            ),
        ];
        for (raw, expected) in cases {
            let err = parse_query_filter(raw).unwrap_err().to_string();
            assert!(err.contains(expected), "{raw}: got {err}");
        }
    }

    #[test]
    fn test_redirects_stay_on_the_stash_server() {
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        let allowed = [
            ("http://stash:9999/graphql", "http://stash:9999/other"),
            ("http://stash.lan/graphql", "https://stash.lan/graphql"),
            ("https://stash.lan/a", "https://stash.lan:443/b"),
        ];
        let refused = [
            ("http://stash:9999/a", "http://evil:9999/a"),
            ("http://stash:9999/a", "http://stash:8080/a"),
            ("https://stash.lan/a", "http://stash.lan/a"),
            ("http://stash.lan/a", "https://stash.lan:8443/a"),
        ];
        for (from, to) in allowed {
            assert!(redirect_allowed(&url(from), &url(to)), "{from} -> {to}");
        }
        for (from, to) in refused {
            assert!(!redirect_allowed(&url(from), &url(to)), "{from} -> {to}");
        }
    }

    #[test]
    fn test_parse_query_filter_ignores_a_byte_order_mark() {
        for raw in [
            "\u{feff}{\"filter\": {\"sort\": \"rating\"}}",
            " \u{feff}{\"filter\": {\"sort\": \"rating\"}}\u{feff}\n",
        ] {
            let parsed = parse_query_filter(raw).unwrap();
            assert_eq!(parsed.filter["sort"], "rating");
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
        clean_wallpaper_cache(dir.path(), &[&dir.path().join("wallpaper_2_0.png")]);

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
        async fn a_missing_content_type_falls_back_to_the_bytes() {
            let server = MockServer::start().await;
            serve(
                &server,
                "/img",
                ResponseTemplate::new(200).set_body_bytes(png_bytes()),
            )
            .await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(
                matches!(result.unwrap(), Download::Saved(p) if p.extension().unwrap() == "png")
            );
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
            // a JPEG header in front of garbage, and a PNG cut short
            let mut damaged = vec![0xFF, 0xD8, 0xFF, 0xE0];
            damaged.extend_from_slice(b"not really a jpeg at all");
            serve(
                &server,
                "/damaged",
                ResponseTemplate::new(200).set_body_raw(damaged, "image/jpeg"),
            )
            .await;
            let png = png_bytes();
            serve(
                &server,
                "/cut",
                ResponseTemplate::new(200).set_body_raw(png[..png.len() / 2].to_vec(), "image/png"),
            )
            .await;

            for (at, reason) in [
                ("/clip", "video/mp4, not an image"),
                ("/page", "text/html, not an image"),
                ("/lie", "isn't a recognizable image"),
                ("/missing", "HTTP 404"),
                ("/broken", "HTTP 500"),
                ("/damaged", "damaged or incomplete"),
                ("/cut", "damaged or incomplete"),
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
        async fn a_huge_image_is_too_large_not_damaged() {
            // a PNG header claiming 40000x40000 RGBA (about 6.4 GB decoded)
            let huge: Vec<u8> = vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
                0x44, 0x52, 0x00, 0x00, 0x9c, 0x40, 0x00, 0x00, 0x9c, 0x40, 0x08, 0x06, 0x00, 0x00,
                0x00, 0x51, 0x0c, 0x0e, 0x05, 0x00, 0x00, 0x00, 0x09, 0x49, 0x44, 0x41, 0x54, 0x78,
                0x9c, 0x63, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x5e, 0xff, 0x7d, 0xf9, 0x00, 0x00,
                0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
            ];
            let server = MockServer::start().await;
            serve(
                &server,
                "/huge",
                ResponseTemplate::new(200).set_body_raw(huge, "image/png"),
            )
            .await;
            let (result, _dir) = download(&server, "/huge").await;
            assert!(matches!(result.unwrap(), Download::Unusable(r) if r.contains("too large")));
        }

        #[tokio::test]
        async fn a_rejected_key_fails_the_rotation() {
            let server = MockServer::start().await;
            serve(&server, "/img", ResponseTemplate::new(401)).await;
            let (result, _dir) = download(&server, "/img").await;
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("API key was rejected"));
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
        async fn find_images_explains_a_redirect_it_cannot_follow() {
            let server = MockServer::start().await;
            // a 301 turns the POST into an empty GET, which Stash rejects
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(
                    ResponseTemplate::new(301).insert_header("Location", "/stash/graphql"),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/stash/graphql"))
                .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                    "errors": [{"message": "no operation provided"}], "data": null
                })))
                .mount(&server)
                .await;
            let settings = settings_for(&server.uri(), "{}");
            let client = client_for(&settings).unwrap();
            let err = query_image_count(&client, &settings)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("redirected to"), "{err}");
            assert!(err.contains("as the Server URL"), "{err}");
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
            assert!(err.to_string().contains("API key was rejected"), "{err}");
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
