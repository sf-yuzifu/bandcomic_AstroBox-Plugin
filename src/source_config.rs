//! Validated /config snapshots, endpoint-owned drafts and immutable sync payloads.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;

pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
const MAX_SOURCES: usize = 128;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_COOKIE_BYTES: usize = 16 * 1024;
const MAX_DRAFTS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Address(String),
    Network(String),
    Http(u16),
    Json(String),
    Model { key: Option<String>, field: String, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(message) => write!(f, "地址错误：{message}"),
            Self::Network(message) => write!(f, "网络错误：{message}"),
            Self::Http(status) => write!(f, "配置接口返回 HTTP {status}"),
            Self::Json(message) => write!(f, "配置 JSON 错误：{message}"),
            Self::Model { key, field, message } => {
                if let Some(key) = key { write!(f, "源「{key}」")?; }
                write!(f, "字段 {field}：{message}")
            }
        }
    }
}

fn model_error(key: Option<&str>, field: &str, message: &str) -> ConfigError {
    ConfigError::Model { key: key.map(str::to_string), field: field.into(), message: message.into() }
}

fn normalize_base_url(input: &str, allow_bare: bool) -> Result<String, ConfigError> {
    let text = input.trim();
    if text.is_empty() { return Err(ConfigError::Address("请输入漫画源 API 地址".into())); }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConfigError::Address("地址不能包含空格或控制字符".into()));
    }
    let candidate = if allow_bare && !text.contains("://") { format!("https://{text}") } else { text.into() };
    let parsed = url::Url::parse(&candidate).map_err(|e| ConfigError::Address(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ConfigError::Address("仅支持带有效主机的 HTTP/HTTPS 地址".into()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ConfigError::Address("基础地址不接受账号、查询参数或片段，请将接口参数放在路径配置中".into()));
    }
    if text.contains('<') || text.contains('>') { return Err(ConfigError::Address("基础地址不能包含占位符".into())); }
    if candidate.len() > 8192 { return Err(ConfigError::Address("地址过长".into())); }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

pub fn normalize_endpoint(input: &str) -> Result<String, ConfigError> { normalize_base_url(input, true) }

fn string_field<'a>(key: &str, config: &'a Value, field: &str, max: usize) -> Result<&'a str, ConfigError> {
    let value = config.get(field).and_then(Value::as_str).filter(|v| !v.trim().is_empty())
        .ok_or_else(|| model_error(Some(key), field, "需要非空字符串"))?;
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(model_error(Some(key), field, "内容过长或包含控制字符"));
    }
    Ok(value)
}

fn validate_path(key: &str, config: &Value, field: &str, required: &[&str], allowed: &[&str]) -> Result<(), ConfigError> {
    let path = string_field(key, config, field, 8192)?;
    if !path.starts_with('/') || path.starts_with("//") || path.contains('#') || path.contains('\\') || path.chars().any(char::is_whitespace) {
        return Err(model_error(Some(key), field, "需要以单个 / 开头的接口路径，不接受空格、片段或完整 URL"));
    }
    for placeholder in required {
        if !path.contains(&format!("<{placeholder}>")) {
            return Err(model_error(Some(key), field, &format!("缺少 <{placeholder}> 占位符")));
        }
    }
    let mut rest = path;
    while let Some(start) = rest.find('<') {
        if rest[..start].contains('>') { return Err(model_error(Some(key), field, "占位符格式错误")); }
        let Some(end) = rest[start + 1..].find('>') else { return Err(model_error(Some(key), field, "占位符未闭合")); };
        let token = &rest[start + 1..start + 1 + end];
        if !allowed.contains(&token) { return Err(model_error(Some(key), field, &format!("不支持 <{token}> 占位符"))); }
        rest = &rest[start + end + 2..];
    }
    if rest.contains('>') { return Err(model_error(Some(key), field, "占位符格式错误")); }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub key: String,
    pub name: String,
    pub api_url: String,
    pub config: Value,
    pub error: Option<ConfigError>,
}

fn validate_entry(key: &str, config: &mut Value) -> Result<(String, String), ConfigError> {
    if key.trim().is_empty() || key.trim() != key || key.len() > 512 || key.chars().any(char::is_control) || key.contains('/') || key.contains('\\') ||
        matches!(key, "using" | "type" | "__proto__" | "constructor" | "prototype") {
        return Err(model_error(Some(key), "key", "源 key 为空、非法或与设备/消息保留字段冲突"));
    }
    if !config.is_object() { return Err(model_error(Some(key), "config", "源配置需要为对象")); }
    let name = string_field(key, config, "name", 4096)?.trim().to_string();
    let api = string_field(key, config, "apiUrl", 8192)?;
    let api_url = normalize_base_url(api, false).map_err(|e| model_error(Some(key), "apiUrl", &e.to_string()))?;
    validate_path(key, config, "detailPath", &["id"], &["id"])?;
    validate_path(key, config, "photoPath", &["id"], &["id", "chapter"])?;
    validate_path(key, config, "searchPath", &["text", "page"], &["text", "page"])?;
    if config.get("type").is_some() { string_field(key, config, "type", 512)?; }
    config["apiUrl"] = json!(api_url);
    Ok((name, api_url))
}

#[derive(Debug, Clone)]
pub struct SourceCatalog {
    pub endpoint: String,
    pub entries: Vec<SourceEntry>,
}

impl SourceCatalog {
    pub fn valid_count(&self) -> usize { self.entries.iter().filter(|e| e.error.is_none()).count() }
}

pub fn parse_catalog(endpoint: &str, body: &[u8]) -> Result<SourceCatalog, ConfigError> {
    if body.len() > MAX_CONFIG_BYTES { return Err(model_error(None, "config", "响应超过 256KiB 限制")); }
    let value: Value = serde_json::from_slice(body).map_err(|e| ConfigError::Json(e.to_string()))?;
    let object = value.as_object().filter(|o| !o.is_empty()).ok_or_else(|| model_error(None, "config", "需要非空的 key → 配置对象"))?;
    if object.len() > MAX_SOURCES { return Err(model_error(None, "config", "配置超过 128 个源")); }
    let entries = object.iter().map(|(key, raw)| {
        let mut config = raw.clone();
        let validation = validate_entry(key, &mut config);
        let (name, api_url, error) = match validation {
            Ok((name, api_url)) => (name, api_url, None),
            Err(error) => (raw.get("name").and_then(Value::as_str).unwrap_or(key).into(),
                raw.get("apiUrl").and_then(Value::as_str).unwrap_or("").into(), Some(error)),
        };
        SourceEntry { key:key.clone(), name, api_url, config, error }
    }).collect();
    Ok(SourceCatalog { endpoint: normalize_endpoint(endpoint)?, entries })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CookieAction { #[default] Keep, Update, Clear }

#[derive(Debug, Clone, Default)]
pub struct SourceChoice {
    pub selected: bool,
    pub cookie_action: CookieAction,
    pub cookie: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CatalogPhase { #[default] Empty, Dirty, Loading, Ready, Error }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadTicket { pub generation: u64, pub endpoint: String }

#[derive(Default)]
pub struct SourceForm {
    pub input: String,
    pub generation: u64,
    pub endpoint: Option<String>,
    pub phase: CatalogPhase,
    pub message: String,
    pub catalog: Option<SourceCatalog>,
    pub choices: Vec<SourceChoice>,
    pub page: usize,
    // In-run drafts only; source persistence belongs to P3-26. apiUrl changes get a new identity.
    drafts: HashMap<(String, String, String), SourceChoice>,
}

impl SourceForm {
    pub fn set_input(&mut self, input: String) -> bool {
        let endpoint = normalize_endpoint(&input).ok();
        self.input = input;
        if endpoint == self.endpoint && endpoint.is_some() { return false; }
        self.endpoint = endpoint;
        self.generation = self.generation.wrapping_add(1);
        self.catalog = None; self.choices.clear(); self.page = 0;
        self.phase = if self.input.trim().is_empty() { CatalogPhase::Empty } else { CatalogPhase::Dirty };
        self.message = if self.phase == CatalogPhase::Empty { "请输入漫画源 API 地址" } else { "地址已修改，请读取新配置" }.into();
        true
    }

    pub fn begin_load(&mut self, force: bool) -> Result<Option<LoadTicket>, ConfigError> {
        let endpoint = normalize_endpoint(&self.input)?;
        if self.phase == CatalogPhase::Loading || (!force && matches!(self.phase, CatalogPhase::Ready | CatalogPhase::Error) && self.endpoint.as_ref() == Some(&endpoint)) { return Ok(None); }
        self.generation = self.generation.wrapping_add(1);
        self.endpoint = Some(endpoint.clone()); self.catalog = None; self.choices.clear(); self.page = 0;
        self.phase = CatalogPhase::Loading; self.message = "正在读取 /config…".into();
        Ok(Some(LoadTicket { generation:self.generation, endpoint }))
    }

    pub fn apply_load(&mut self, ticket: &LoadTicket, result: Result<SourceCatalog, ConfigError>) -> bool {
        if ticket.generation != self.generation || self.endpoint.as_ref() != Some(&ticket.endpoint) || self.phase != CatalogPhase::Loading { return false; }
        match result {
            Ok(catalog) if catalog.endpoint == ticket.endpoint => {
                let valid = catalog.valid_count();
                let single = catalog.entries.len() == 1;
                self.choices = catalog.entries.iter().map(|entry| {
                    if entry.error.is_some() { return SourceChoice::default(); }
                    self.drafts.get(&(catalog.endpoint.clone(), entry.key.clone(), entry.api_url.clone())).cloned()
                        .unwrap_or(SourceChoice { selected:single, ..Default::default() })
                }).collect();
                self.phase = if valid > 0 { CatalogPhase::Ready } else { CatalogPhase::Error };
                self.message = format!("已读取 {} 个源，{} 个可同步；{} 个配置无效", catalog.entries.len(), valid, catalog.entries.len() - valid);
                self.catalog = Some(catalog);
            }
            Ok(_) => return false,
            Err(error) => { self.phase = CatalogPhase::Error; self.message = error.to_string(); self.catalog = None; self.choices.clear(); }
        }
        true
    }

    pub fn edit_choice(&mut self, generation: u64, index: usize, edit: impl FnOnce(&mut SourceChoice)) -> bool {
        if generation != self.generation || self.phase != CatalogPhase::Ready { return false; }
        let Some(catalog) = &self.catalog else { return false; };
        let Some(entry) = catalog.entries.get(index).filter(|e| e.error.is_none()) else { return false; };
        let Some(choice) = self.choices.get_mut(index) else { return false; };
        edit(choice);
        let identity = (catalog.endpoint.clone(), entry.key.clone(), entry.api_url.clone());
        if self.drafts.len() >= MAX_DRAFTS && !self.drafts.contains_key(&identity) {
            if let Some(key) = self.drafts.keys().find(|k| k.0 != catalog.endpoint).cloned().or_else(|| self.drafts.keys().next().cloned()) { self.drafts.remove(&key); }
        }
        self.drafts.insert(identity, choice.clone());
        self.message = format!("已读取 {} 个源，{} 个可同步；{} 个配置无效", catalog.entries.len(), catalog.valid_count(), catalog.entries.len() - catalog.valid_count());
        true
    }

    pub fn selected_count(&self) -> usize { self.choices.iter().filter(|c| c.selected).count() }

    pub fn build_plan(&self) -> Result<SyncPlan, ConfigError> {
        let catalog = self.catalog.as_ref().filter(|c| self.phase == CatalogPhase::Ready && self.endpoint.as_ref() == Some(&c.endpoint) && normalize_endpoint(&self.input).ok().as_ref() == Some(&c.endpoint))
            .ok_or_else(|| model_error(None, "config", "请先成功读取当前地址的配置"))?;
        let mut configs = Vec::new(); let mut keys = Vec::new(); let mut cookies = serde_json::Map::new();
        for (index, choice) in self.choices.iter().enumerate().filter(|(_, c)| c.selected) {
            let entry = catalog.entries.get(index).ok_or_else(|| model_error(None, "selection", "源列表已变化"))?;
            if let Some(error) = &entry.error { return Err(error.clone()); }
            keys.push(entry.key.clone());
            configs.push(json!({&entry.key:entry.config}));
            match choice.cookie_action {
                CookieAction::Keep => {},
                CookieAction::Clear => { cookies.insert(entry.key.clone(), json!("")); },
                CookieAction::Update => {
                    if choice.cookie.trim().is_empty() { return Err(model_error(Some(&entry.key), "Cookie", "更新需要非空内容；清空请明确选择清空操作")); }
                    if choice.cookie.len() > MAX_COOKIE_BYTES || choice.cookie.chars().any(char::is_control) { return Err(model_error(Some(&entry.key), "Cookie", "内容超过 16KiB 或包含换行/控制字符")); }
                    cookies.insert(entry.key.clone(), json!(choice.cookie));
                }
            }
        }
        if configs.is_empty() { return Err(model_error(None, "selection", "请勾选至少一个有效漫画源")); }
        let configs_message = json!({"type":"source_config","configs":configs}).to_string();
        let cookie_count = cookies.len();
        let cookie_message = if cookies.is_empty() { None } else { cookies.insert("type".into(), json!("cookie")); Some(Value::Object(cookies).to_string()) };
        if configs_message.len() > MAX_MESSAGE_BYTES || cookie_message.as_ref().is_some_and(|m| m.len() > MAX_MESSAGE_BYTES) {
            return Err(model_error(None, "payload", "控制消息超过 64KiB，请减少一次选择的源"));
        }
        Ok(SyncPlan { generation:self.generation, endpoint:catalog.endpoint.clone(), keys, configs_message, cookie_message, cookie_count })
    }

    pub fn plan_current(&self, plan: &SyncPlan) -> bool { self.generation == plan.generation && self.endpoint.as_ref() == Some(&plan.endpoint) && self.phase == CatalogPhase::Ready }
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub generation: u64,
    pub endpoint: String,
    pub keys: Vec<String>,
    pub configs_message: String,
    pub cookie_message: Option<String>,
    pub cookie_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPhase { Preparing, Sending, Sent, Partial, Failed, Cancelled }

impl SyncPhase { pub fn busy(self) -> bool { matches!(self, Self::Preparing | Self::Sending) } }

#[derive(Debug, Clone)]
pub struct SourceSync {
    pub id: u64,
    pub plan: SyncPlan,
    pub device_name: String,
    pub device_addr: String,
    pub phase: SyncPhase,
    pub configs_sent: bool,
    pub cookies_sent: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(api: &str) -> Value {
        json!({"name":"同名源", "apiUrl":api, "detailPath":"/album/<id>",
            "photoPath":"/photo/<id>", "searchPath":"/search?q=<text>&page=<page>",
            "future":{"enabled":true}})
    }
    fn load(form: &mut SourceForm, endpoint: &str, value: Value) -> LoadTicket {
        form.set_input(endpoint.into());
        let ticket = form.begin_load(true).unwrap().unwrap();
        assert!(form.apply_load(&ticket, parse_catalog(endpoint, &serde_json::to_vec(&value).unwrap())));
        ticket
    }

    #[test]
    fn endpoint_normalization_supports_http_localhost_ports_prefixes_and_bare_addresses() {
        for (input, expected) in [(" example.com/ ","https://example.com"),
            ("http://localhost:1234/api/","http://localhost:1234/api"),
            ("http://127.0.0.1:1234/","http://127.0.0.1:1234"),
            ("https://[::1]:1234/api/","https://[::1]:1234/api"),
            ("HTTPS://EXAMPLE.COM:443/","https://example.com")] {
            assert_eq!(normalize_endpoint(input).unwrap(),expected);
        }
        for input in ["", "ftp://example.com", "http://localhost:99999", "http://999.1.2.3", "http://a/?token=1", "http://a/#x", "http://user:password@a", "http://exa mple.com", "http://a/\n/path"] {
            assert!(matches!(normalize_endpoint(input),Err(ConfigError::Address(_))),"{input}");
        }
    }

    #[test]
    fn catalog_keeps_key_distinct_from_name_optional_type_single_photo_and_unknown_extensions() {
        let catalog = parse_catalog("http://localhost:1234", &serde_json::to_vec(&json!({"A":config("http://a/"),"B":config("http://b/api/")})).unwrap()).unwrap();
        assert_eq!(catalog.entries.len(),2); assert_eq!(catalog.valid_count(),2);
        assert_eq!(catalog.entries[0].name,catalog.entries[1].name);
        assert_eq!(catalog.entries[0].key,"A"); assert_eq!(catalog.entries[1].key,"B");
        assert_eq!(catalog.entries[1].config["apiUrl"],"http://b/api");
        assert_eq!(catalog.entries[1].config["future"]["enabled"],true);
        assert!(catalog.entries[0].config.get("type").is_none());
    }

    #[test]
    fn json_root_model_and_field_errors_are_distinct_and_bad_entries_cannot_be_selected() {
        assert!(matches!(parse_catalog("http://a",b"{broken"),Err(ConfigError::Json(_))));
        for body in [b"[]".as_slice(),b"{}",b"null",b"42"] {
            assert!(matches!(parse_catalog("http://a",body),Err(ConfigError::Model {..})));
        }
        let mut bad = config("http://bad"); bad["detailPath"] = json!("/album");
        let mut form = SourceForm::default(); let ticket = load(&mut form,"http://a",json!({"A":config("http://a"),"Bad":bad}));
        assert_eq!(form.phase,CatalogPhase::Ready); assert_eq!(form.catalog.as_ref().unwrap().valid_count(),1);
        assert!(!form.edit_choice(ticket.generation,1,|c|c.selected=true));
        let error = form.catalog.as_ref().unwrap().entries[1].error.as_ref().unwrap().to_string();
        assert!(error.contains("Bad") && error.contains("detailPath"));
        assert!(form.edit_choice(ticket.generation,0,|c|c.selected=true));
        assert_eq!(form.build_plan().unwrap().keys,vec!["A"]);
    }

    #[test]
    fn reserved_keys_invalid_url_type_and_unsupported_placeholders_have_field_errors() {
        for key in ["using","type","__proto__","constructor","prototype","bad/key","bad\\key"] {
            let catalog = parse_catalog("http://a", &serde_json::to_vec(&json!({key:config("http://a")})).unwrap()).unwrap();
            assert_eq!(catalog.valid_count(),0);
        }
        for (field,value) in [("apiUrl",json!("example.com")),("type",json!(123)),
            ("photoPath",json!("https://a/photo/<id>")),("photoPath",json!("/photo/<id>/<unsupported>")),
            ("searchPath",json!("/search/<text>")),("detailPath",json!("/album/<id"))] {
            let mut entry = config("http://a"); entry[field] = value;
            let catalog = parse_catalog("http://a", &serde_json::to_vec(&json!({"A":entry})).unwrap()).unwrap();
            assert!(matches!(&catalog.entries[0].error,Some(ConfigError::Model {field:error_field,..}) if error_field == field));
        }
    }

    #[test]
    fn single_source_defaults_selected_multi_source_and_all_invalid_never_silently_send() {
        let mut form = SourceForm::default();
        load(&mut form,"http://a",json!({"A":config("http://a")}));
        assert_eq!(form.selected_count(),1); assert!(form.build_plan().unwrap().cookie_message.is_none());
        load(&mut form,"http://a",json!({"A":config("http://a"),"B":config("http://b")}));
        assert_eq!(form.selected_count(),0); assert!(form.build_plan().is_err());
        load(&mut form,"http://a",json!({"Bad":null}));
        assert_eq!(form.phase,CatalogPhase::Error); assert!(form.build_plan().is_err());
        assert!(form.catalog.as_ref().unwrap().entries[0].error.is_some());
    }

    #[test]
    fn reading_coalesces_same_endpoint_and_requires_explicit_refresh_after_success_or_failure() {
        let mut form = SourceForm::default(); form.set_input("http://a/".into());
        let ticket = form.begin_load(false).unwrap().unwrap();
        assert!(form.begin_load(false).unwrap().is_none());
        form.apply_load(&ticket,parse_catalog("http://a",&serde_json::to_vec(&json!({"A":config("http://a")})).unwrap()));
        assert!(!form.set_input(" http://a ".into()));
        assert!(form.begin_load(false).unwrap().is_none());
        let refreshed = form.begin_load(true).unwrap().unwrap(); assert_ne!(refreshed.generation,ticket.generation);
        form.apply_load(&refreshed,Err(ConfigError::Http(503)));
        assert!(form.begin_load(false).unwrap().is_none()); assert!(form.build_plan().is_err());
        assert!(form.message.contains("HTTP 503"));
        assert!(form.begin_load(true).unwrap().is_some());
    }

    #[test]
    fn endpoint_change_invalidates_ready_data_and_late_response_even_after_switching_back() {
        let mut form = SourceForm::default(); let old = load(&mut form,"http://a",json!({"A":config("http://a")}));
        let plan = form.build_plan().unwrap();
        form.set_input("http://b".into());
        assert_eq!(form.phase,CatalogPhase::Dirty); assert!(form.catalog.is_none());
        assert!(!form.plan_current(&plan)); assert!(form.build_plan().is_err());
        let pending = form.begin_load(false).unwrap().unwrap();
        form.set_input("http://a".into()); let current = form.begin_load(false).unwrap().unwrap();
        let a = parse_catalog("http://a",&serde_json::to_vec(&json!({"A":config("http://a")})).unwrap()).unwrap();
        assert!(!form.apply_load(&old,Ok(a.clone())));
        assert!(!form.apply_load(&pending,Err(ConfigError::Network("old error".into()))));
        assert!(form.apply_load(&current,Ok(a)));
        assert!(form.build_plan().is_ok());
    }

    #[test]
    fn cookie_keep_update_clear_and_unselected_drafts_produce_only_selected_legacy_fields() {
        let mut form = SourceForm::default(); let ticket = load(&mut form,"http://a",json!({"A":config("http://a"),"B":config("http://b"),"C":config("http://c")}));
        form.edit_choice(ticket.generation,0,|c| { c.selected=true; c.cookie_action=CookieAction::Update; c.cookie="a=1; token=A".into(); });
        form.edit_choice(ticket.generation,1,|c| { c.selected=true; c.cookie_action=CookieAction::Clear; c.cookie="old-b".into(); });
        form.edit_choice(ticket.generation,2,|c| { c.cookie_action=CookieAction::Update; c.cookie="unselected-c".into(); });
        let plan = form.build_plan().unwrap();
        assert_eq!(plan.keys,vec!["A","B"]);
        let cookies: Value = serde_json::from_str(plan.cookie_message.as_ref().unwrap()).unwrap();
        assert_eq!(cookies,json!({"type":"cookie","A":"a=1; token=A","B":""}));
        let configs: Value = serde_json::from_str(&plan.configs_message).unwrap();
        assert_eq!(configs["configs"].as_array().unwrap().len(),2);
        assert_eq!(configs["configs"][0].as_object().unwrap().len(),1);
        form.edit_choice(ticket.generation,0,|c|c.cookie_action=CookieAction::Keep);
        let kept: Value = serde_json::from_str(form.build_plan().unwrap().cookie_message.as_ref().unwrap()).unwrap();
        assert!(kept.get("A").is_none()); assert_eq!(kept["B"],"");
    }

    #[test]
    fn cookies_are_owned_by_endpoint_key_and_api_identity_and_not_inherited_by_other_deployments() {
        let mut form = SourceForm::default(); let a = load(&mut form,"http://config-a",json!({"A":config("http://api-a")}));
        form.edit_choice(a.generation,0,|c| { c.cookie_action=CookieAction::Update; c.cookie="for-a".into(); });
        load(&mut form,"http://config-b",json!({"A":config("http://api-a")}));
        assert!(form.choices[0].cookie.is_empty()); assert_eq!(form.choices[0].cookie_action,CookieAction::Keep);
        load(&mut form,"http://config-a",json!({"A":config("http://api-a")}));
        assert_eq!(form.choices[0].cookie,"for-a");
        load(&mut form,"http://config-a",json!({"A":config("http://api-new")}));
        assert!(form.choices[0].cookie.is_empty());
    }

    #[test]
    fn old_choice_events_cannot_write_into_a_new_catalog_slot() {
        let mut form = SourceForm::default(); let old = load(&mut form,"http://a",json!({"A":config("http://a")}));
        load(&mut form,"http://a",json!({"B":config("http://b")}));
        assert!(!form.edit_choice(old.generation,0,|c| { c.cookie_action=CookieAction::Update; c.cookie="old-a".into(); }));
        assert!(form.choices[0].cookie.is_empty());
        assert_eq!(form.build_plan().unwrap().keys,vec!["B"]);
    }

    #[test]
    fn a_built_sync_plan_stays_immutable_when_selection_and_cookie_drafts_change() {
        let mut form = SourceForm::default(); let ticket = load(&mut form,"http://a",json!({"A":config("http://a")}));
        form.edit_choice(ticket.generation,0,|c| { c.cookie_action=CookieAction::Update; c.cookie="first".into(); });
        let plan = form.build_plan().unwrap();
        form.edit_choice(ticket.generation,0,|c| { c.selected=false; c.cookie="second".into(); });
        assert_eq!(plan.keys,vec!["A"]);
        assert_eq!(serde_json::from_str::<Value>(plan.cookie_message.as_ref().unwrap()).unwrap()["A"],"first");
    }

    #[test]
    fn empty_control_or_oversized_cookie_updates_block_entire_plan_before_any_send() {
        let mut form = SourceForm::default(); let ticket = load(&mut form,"http://a",json!({"A":config("http://a")}));
        for cookie in ["".to_string()," \t ".into(),"a=1\r\nBad: x".into(),"x".repeat(MAX_COOKIE_BYTES+1)] {
            form.edit_choice(ticket.generation,0,|c| { c.cookie_action=CookieAction::Update; c.cookie=cookie; });
            assert!(matches!(form.build_plan(),Err(ConfigError::Model {field,..}) if field == "Cookie"));
        }
        form.edit_choice(ticket.generation,0,|c|c.cookie_action=CookieAction::Clear);
        assert_eq!(serde_json::from_str::<Value>(form.build_plan().unwrap().cookie_message.as_ref().unwrap()).unwrap()["A"],"");
    }

    #[test]
    fn response_and_message_budgets_fail_explicitly_instead_of_dropping_selected_sources() {
        assert!(matches!(parse_catalog("http://a",&vec![b' ';MAX_CONFIG_BYTES+1]),Err(ConfigError::Model {..})));
        let mut large = config("http://a"); large["future"] = json!("x".repeat(MAX_MESSAGE_BYTES));
        let mut form = SourceForm::default(); load(&mut form,"http://a",json!({"A":large}));
        assert!(matches!(form.build_plan(),Err(ConfigError::Model {field,..}) if field == "payload"));
    }
}
