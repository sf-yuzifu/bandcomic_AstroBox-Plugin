use serde_json::Value;
use waki::Client;

async fn fetch_config_raw(domain: &str) -> Option<Value> {
    let normalized_domain = domain.trim_end_matches('/');
    let config_url = format!("{}/config", normalized_domain);

    tracing::info!("请求配置 URL: {}", config_url);

    let client = Client::new();
    let resp = match client
        .get(&config_url)
        .header("Content-Type", "application/json")
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("请求失败: {}", e);
            return None;
        }
    };

    if resp.status_code() != 200 {
        tracing::error!("获取配置失败，状态码: {}", resp.status_code());
        return None;
    }

    let body: Vec<u8> = match resp.body() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("读取响应体失败: {}", e);
            return None;
        }
    };

    let body_str = String::from_utf8_lossy(&body);
    tracing::info!("响应体: {}", body_str);

    match serde_json::from_str::<Value>(&body_str) {
        Ok(config_data) => Some(config_data),
        Err(e) => {
            tracing::error!("解析配置 JSON 失败: {}", e);
            None
        }
    }
}

pub async fn fetch_source_name(domain: &str) -> Option<String> {
    let config_data = fetch_config_raw(domain).await?;

    let source_names: Vec<&str> = config_data
        .as_object()
        .map(|obj| obj.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    tracing::info!("配置中的 sourceNames: {:?}", source_names);

    if source_names.is_empty() {
        tracing::error!("配置中没有找到 sourceName");
        return None;
    }

    let source_name = source_names[0].to_string();
    tracing::info!("成功获取 sourceName: {}", source_name);
    Some(source_name)
}

pub async fn fetch_source_config(domain: &str) -> Option<Value> {
    let config_data = fetch_config_raw(domain).await?;

    let source_names: Vec<&str> = config_data
        .as_object()
        .map(|obj| obj.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    if source_names.is_empty() {
        tracing::error!("配置中没有找到 sourceName");
        return None;
    }

    let config_array: Vec<Value> = source_names
        .iter()
        .map(|name| {
            let name = name.to_string();
            let value = &config_data[&name];
            serde_json::json!({ name: value })
        })
        .collect();

    tracing::info!("成功获取完整配置，共 {} 个源", config_array.len());
    Some(Value::Array(config_array))
}
