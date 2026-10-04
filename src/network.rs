use crate::source_config::{ConfigError, SourceCatalog, normalize_endpoint, parse_catalog};
use waki::Client;

/// One response is the authority for both display names and complete source configs.
pub async fn fetch_source_catalog(endpoint: &str) -> Result<SourceCatalog, ConfigError> {
    let endpoint = normalize_endpoint(endpoint)?;
    let config_url = format!("{endpoint}/config");
    tracing::info!("读取漫画源配置: {}", config_url);
    let response = Client::new().get(&config_url).header("Accept", "application/json")
        .connect_timeout(std::time::Duration::from_secs(15)).send()
        .map_err(|e| ConfigError::Network(e.to_string()))?;
    if response.status_code() != 200 { return Err(ConfigError::Http(response.status_code())); }
    let body = response.body().map_err(|e| ConfigError::Network(format!("读取响应失败：{e}")))?;
    let catalog = parse_catalog(&endpoint, &body)?;
    tracing::info!("配置已解析: {} 个源，{} 个有效", catalog.entries.len(), catalog.valid_count());
    Ok(catalog)
}
