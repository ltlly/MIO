# MIO Scripting API Specification (Rhai / MioScript Engine)

Authoritative specification matching `crates/mio-core/src/http/mio_script/`.

## 1. Engine & Runtime Model

- **Language**: **Rhai `1.27`** (`let`, `const`, `fn`, `if/else`, `for ... in`, `#{ ... }` map literals, `[...]` arrays, `` `... ${expr}` `` interpolation, `print(...)`). No Java/JDK imports exist; helpers are registered directly on Rhai `Map`s and `utilities`.
- **Regex Flavor**: Rust `regex` crate (linear-time DFA; no lookaround or backreferences). Pattern cache: `256` entries, `4,096` bytes max per pattern.
- **Execution Limits**:

| Limit | Value | Constant / Enforcement |
|---|---|---|
| Max operations | `50,000` | `engine.set_max_operations(50_000)` |
| Max string size | `16 MiB` (`16 * 1024 * 1024`) | `MAX_BODY` |
| Max array / map entries | `4,096` / `4,096` | `set_max_array_size` / `set_max_map_size` |
| Max call / expr depth | `32` calls, `64/32` expr | `set_max_call_levels(32)` |
| Max script size | `64 KiB` (`64 * 1024`) | `script_keyword_ranges` / entrypoints |
| JSON recursion depth | `32` | `MAX_JSON_SCRIPT_DEPTH` |

---

## 2. Script Entry Points (5 Total)

| Entry Point | Rust API (`mio_core`) | Scope Variables | Return & Side-Effect Semantics |
|---|---|---|---|
| **HTTP History / Logger Filter** | `eval_http_filter_script*` | `request`, `response`, `requestResponse`, `annotations`, `utilities`, `search_text`, `notes`, `highlighted`, `listener_port`, `in_scope`, `parameterized`, `has_response`, `mime_index` | Must return **`bool`**. Cancellable via `CancellationToken`. AST cached (`128`). |
| **WebSocket History Filter** | `eval_ws_filter_script*` | `message`, `in_scope` | Must return **`bool`**. `message.direction` is `"c2s"` or `"s2c"`. |
| **Match & Replace (`RequestScript` / `ResponseScript`)** | `execute_http_match_replace_script` | `request`, `response` | Mutate `request`/`response` in place or return modified `Map`, raw HTTP `String`, or `()` (`unit`). Syncs changes into `FlowRecord` + `Content-Length`. |
| **Repeater Custom Actions** | `eval_custom_action_with_mutation` | `request`, `response`, `requestResponse`, `utilities` | Captures up to `256` `print(...)` lines + non-unit/non-request return value into `outcome.output`. If `request` map (or returned request map) differs from initial snapshot, applies diff via `apply_request_map` and sets `outcome.updated_request = Some(...)` (synced live into active Repeater editor). |
| **Custom Scan Checks** | `eval_custom_scan_check_script` | `request`, `response`, `requestResponse`, `utilities` | Hit if return value is: `true`, non-empty `String` (used as evidence), `#{ matched: true, evidence: "..." }` (supports `hit`/`detail` aliases), or non-empty non-`"false"`/`"0"` value. `()` or `false` = miss. |

---

## 3. Injected Object Schemas (`rhai::Map`)

### `request` Map

| Key | Type | Notes |
|---|---|---|
| `method` | `String` | HTTP method (e.g. `"GET"`, `"POST"`) |
| `url` | `String` | Full URL (`http(s)://...`) |
| `path` | `String` | URL path without query |
| `path_with_query` | `String` | URL path + `?query` if present |
| `query` | `String` | Raw query string without leading `?` |
| `host` | `String` | URL host |
| `version` | `String` | `"HTTP/1.0"`, `"HTTP/1.1"`, or `"HTTP/2"` |
| `headers` | `Map<String, String>` | Header map (mutating `request.headers["K"] = "V"` or calling `setHeader`/`removeHeader` updates request) |
| `body` | `String` | UTF-8 lossy body string |
| `raw` | `String` | Full serialized HTTP message. **Precedence**: if `request.raw` is changed, `apply_request_map` re-parses `raw` and ignores granular field edits; otherwise granular fields (`method`/`url`/`path`/`query`/`version`/`headers`/`body`) apply and auto-sync `Content-Length`. |
| `in_scope`, `parameterized` | `bool` | Present in HTTP History filter context |

### `response` Map

| Key | Type | Notes |
|---|---|---|
| `status` | `i64` | `0` when absent; `100..=599` when present |
| `version` | `String` | `"HTTP/1.0"`, `"HTTP/1.1"`, `"HTTP/2"`, or `""` |
| `headers` | `Map<String, String>` | Response headers |
| `body` | `String` | UTF-8 lossy response body |
| `raw` | `String` | Full serialized HTTP response (same precedence as `request.raw` in `ResponseScript`) |
| `length`, `mime_type` | `i64`, `String` | Present in HTTP History filter context |

### `annotations` / `requestResponse` / `message`

- **`annotations`**: `notes` (`String`), `highlighted` (`bool`), `highlight_color` (`String`), `highlight_color_unknown` (`bool`).
- **`requestResponse`**: `request`, `response`, `annotations` (in HTTP filter), plus `method`, `url`, `path`, `path_with_query`, `query`, `host`, `status`, `mime_type`, `notes`, `highlighted`, `highlight_color`, `highlight_color_unknown`, `listener_port`, `in_scope`, `parameterized`, `has_response`, `search_text` when in HTTP filter.
- **`message`** (WS filter): `url` (`String`), `direction` (`"c2s"` | `"s2c"`), `payload` (`String`), `length` (`i64`), `edited` (`bool`), `notes` (`String`), `listener_port` (`i64`), `in_scope` (`bool`).

---

## 4. Complete Registered Functions & Helper Methods

All methods registered with `&mut Map` receiver can be invoked either as `obj.method(...)` or as `method(obj, ...)` where exported.

### Read & Inspection Methods

| Name / Alias | Receiver | Return | Behavior |
|---|---|---|---|
| `request()` / `response()` / `annotations()` | `requestResponse` | `Map` | Nested sub-map |
| `hasResponse()` / `isInScope()` / `hasParameters()` | `requestResponse` / `request` | `bool` | Presence / scope / param flags |
| `method()` / `url()` / `host()` / `httpVersion()` | `request` / `requestResponse` | `String` | HTTP fields (`httpVersion` reads `version`) |
| `path()` | `request` / `requestResponse` | `String` | Returns `path_with_query` if non-empty, else `path` |
| `pathWithoutQuery()` / `query()` | `request` / `requestResponse` | `String` | Path only / query only |
| `bodyToString()` | `request` / `response` | `String` | Returns `body` field |
| `statusCode()` | `response` / `requestResponse` | `i64` | HTTP status code |
| `mimeType()` | `response` / `requestResponse` | `String` | Enum string (see Section 5) |
| `listenerPort()` | `requestResponse` | `i64` | Listening port (`0` if unset) |
| `notes()` / `hasNotes()` | `annotations` / `requestResponse` | `String` / `bool` | Notes text & non-empty check |
| `highlightColor()` / `hasHighlightColor()` | `annotations` / `requestResponse` | `String` / `bool` | Enum string (see Section 5); errors if unknown color code |
| `headerValue(name)` / `header_value(map, name)` | `request` / `response` / `headers` | `String` | Case-insensitive header lookup (`""` if missing) |
| `hasHeader(name)` / `has_header(map, name)` | `request` / `response` / `headers` | `bool` | Case-insensitive header existence |
| `parameterValue(name)` / `hasParameter(name)` | `request` | `String` / `bool` | Checks URL query params AND `application/x-www-form-urlencoded` body |
| `query_param(request, name)` | `request` | `String` | First matching URL query param value or `""` |
| `parseCookies()` / `parse_cookies(map)` | `request` | `Map<String, String>` | Parses `Cookie` header into key/value map |
| `formParam(name)` / `form_param(map, name)` | `request` / `response` | `String` | First matching form-urlencoded body field or `""` |
| `contains(pattern)` / `contains(pattern, case_sensitive)` | `request` / `response` / `requestResponse` | `bool` | Substring search against `search_text` or `raw` |

### Request / Response Mutation Methods

All header, cookie, query, and form mutations automatically keep URL / `path` / `path_with_query` / `query` / `Cookie` / `body` synchronized inside the map (`apply_request_map` recomputes `Content-Length`).

| Name & Alias | Signature | Behavior |
|---|---|---|
| `setHeader` / `set_header` | `(map, name, value) -> ()` | Replaces header case-insensitively (validates HTTP token & bytes) |
| `removeHeader` / `remove_header` | `(map, name) -> ()` | Deletes all headers matching `name` case-insensitively |
| `removeHeaders` / `remove_headers` | `(map, [name1, name2, ...]) -> ()` | Batch remove headers by array of names |
| `removeAllCookies` / `remove_all_cookies` | `(map) -> ()` | Removes `Cookie` header |
| `removeCookie` / `remove_cookie` | `(map, name) -> ()` | Removes single cookie key from `Cookie` header (`"*"` removes header; removes header if last cookie removed) |
| `setCookie` / `set_cookie` | `(map, name, value) -> ()` | Upserts `name=value` inside `Cookie` header |
| `setQueryParam` / `set_query_param` | `(map, name, value) -> ()` | Upserts query param on `url` and syncs `path`, `path_with_query`, `query` |
| `removeQueryParam` / `remove_query_param` | `(map, name) -> ()` | Deletes query param from `url` and syncs `path`, `path_with_query`, `query` |
| `setJsonField` / `set_json_field` | `(map, dot_path, value) -> ()` | Sets nested JSON field (e.g. `"user.role"`, `"items.0.id"`), creating intermediate objects if missing; empty body treated as `{}` |
| `setFormParam` / `set_form_param` | `(map, name, value) -> ()` | Upserts `application/x-www-form-urlencoded` body parameter |
| `removeFormParam` / `remove_form_param` | `(map, name) -> ()` | Removes `application/x-www-form-urlencoded` body parameter |

### Encoding, Crypto, JSON, JWT & Regex Utilities

Available as both free `snake_case(...)` functions AND `utilities.camelCase(...)` methods:

| Free Function (`snake_case`) | `utilities` Method (`camelCase`) | Signature / Output |
|---|---|---|
| `url_encode(text)` | `utilities.urlEncode(text)` | URL form percent-encoded `String` |
| `url_decode(text)` | `utilities.urlDecode(text)` | URL decoded UTF-8 `String` |
| `base64_encode(text)` | `utilities.base64Encode(text)` | Standard Base64 `String` |
| `base64_decode(text)` | `utilities.base64Decode(text)` | Decoded UTF-8 `String` |
| `base64url_encode(text)` | `utilities.base64UrlEncode(text)` | Base64URL without `=` padding |
| `base64url_decode(text)` | `utilities.base64UrlDecode(text)` | Decoded Base64URL UTF-8 `String` (handles `-`/`_`, whitespace, missing `=`) |
| `hex_encode(text)` | `utilities.hexEncode(text)` | Lowercase hex `String` |
| `hex_decode(hex)` | `utilities.hexDecode(hex)` | Strips ASCII whitespace, validates even hex, returns UTF-8 `String` |
| `html_encode(text)` | `utilities.htmlEncode(text)` | Escapes `& < > " '` (`&#39;`) |
| `html_decode(text)` | `utilities.htmlDecode(text)` | Decodes `&amp; &lt; &gt; &quot; &apos; &#NNN; &#xHHH;` |
| `sha256_hex(text)` | `utilities.sha256Hex(text)` | 64-char lowercase SHA-256 hex |
| `md5_hex(text)` | `utilities.md5Hex(text)` | 32-char lowercase MD5 hex |
| `crc32_hex(text)` | — | 8-char lowercase CRC-32 hex |
| `hmac_sha256_hex(key, msg)` | `utilities.hmacSha256Hex(key, msg)` | 64-char lowercase HMAC-SHA256 hex |
| `hmac_sha256_base64(key, msg)` | `utilities.hmacSha256Base64(key, msg)` | Standard Base64 HMAC-SHA256 |
| `hmac_sha256_base64url(key, msg)` | `utilities.hmacSha256Base64Url(key, msg)` | Unpadded Base64URL HMAC-SHA256 |
| `json_parse(text)` | `utilities.jsonParse(text)` | `Dynamic` (`Map` / `Array` / primitives / `()`) |
| `json_stringify(val)` | `utilities.jsonStringify(val)` | Compact JSON `String` |
| `json_stringify_pretty(val)` | `utilities.jsonStringifyPretty(val)` | Pretty JSON `String` |
| `json_get(text, dot_path)` | `utilities.jsonGet(text, dot_path)` | Traverses `"a.b.0.c"`, returns `()` if missing |
| `jwt_decode(token)` | `utilities.jwtDecode(token)` | Strips optional `"Bearer "` prefix; returns `Map #{ header, payload, signature, raw_header, raw_payload }` |
| `regex_matches(text, pattern)` | — | `bool` |
| `regex_replace(text, pattern, repl)` | — | `String` |
| `unix_timestamp()` / `unix_timestamp_ms()` | — | `i64` UNIX seconds / milliseconds |

---

## 5. Enums, Persistence Schemas & Built-in Presets

- **`response.mimeType()` Values**: `JSON`, `HTML`, `SCRIPT`, `XML`, `CSS`, `PLAIN_TEXT`, `IMAGE_PNG`, `IMAGE_JPEG`, `IMAGE_GIF`, `IMAGE_BMP`, `IMAGE_TIFF`, `IMAGE_SVG_XML`, `IMAGE_UNKNOWN`, `FONT_WOFF`, `FONT_WOFF2`, `SSE`, `YAML`, `RTF`, `APPLICATION_FLASH`, `NONE`, `UNRECOGNIZED`.
- **`annotations.highlightColor()` Values**: `NONE` (`0`), `RED` (`1`), `ORANGE` (`2`), `YELLOW` (`3`), `GREEN` (`4`), `CYAN` (`5`), `BLUE` (`6`), `PINK` (`7`), `MAGENTA` (`8`), `GRAY` (`9`).
- **Script Persistence JSON Schema** (`http-filter-scripts.json` & `repeater-custom-actions.json`, `version: 1`, `<= 128` entries, `<= 4 MiB`):
  ```json
  { "version": 1, "scripts": [{ "name": "Script Name", "code": "..." }] }
  ```
- **Repeater Custom Actions Built-in Presets (14)**:
  - **Mutate Request (7)**: `去除 Cookie` (`repeater-custom-action-preset-remove-cookie`), `去除认证头`, `去除缓存头`, `反代绕过头`, `CORS Origin 测试`, `防缓存时间戳`, `精简冗余头`.
  - **Inspect (7)**: `提取 JWT 载荷`, `检查安全响应头`, `提取响应链接`, `解析 Cookie 明细`, `敏感信息扫描`, `生成 cURL 命令`, `统计参数与长度`.

---

## 6. Copy-Paste Minimal Recipes

```rhai
// 1. HTTP History filter: in-scope JSON API errors with param
requestResponse.isInScope() && response.statusCode() >= 400 && response.mimeType() == "JSON" && request.hasParameters()

// 2. WebSocket filter: incoming non-empty JSON frames
message.direction == "s2c" && message.payload.contains("\"token\"")

// 3. Match & Replace (RequestScript): inject HMAC signature header
let ts = unix_timestamp().to_string();
request.setHeader("X-Timestamp", ts);
request.setHeader("X-Sig", hmac_sha256_hex("secret-key", ts + ":" + request.body));

// 4. Repeater Custom Action: strip auth & elevate JSON field
request.removeAllCookies();
request.setJsonField("user.role", "admin");
print("Updated body len=" + request.body.len());

// 5. Custom Scan Check: inspect JWT alg=none inside Authorization
let auth = request.headerValue("Authorization");
if auth.contains(".") {
    let jwt = jwt_decode(auth);
    if jwt.header.alg == "none" { return "JWT uses alg=none: " + jwt.raw_header; }
}
false
```

---

## 7. Known Limitations

- **Lossy UTF-8 body mapping**: `request.body`, `response.body`, and `raw` are materialized via `String::from_utf8_lossy`. Mutating binary/non-UTF-8 payloads (images, protobuf, compressed bytes with unpacking off) through MioScript will corrupt invalid UTF-8 sequences.
- **Deterministic sandboxed bounds**: Scripts are strictly bounded (`50,000` ops, `16 MiB` string / `MAX_BODY`, `64 KiB` source size, `4,096` collection entries) and have zero network or filesystem access (use Native Plugins for I/O or out-of-band traffic).
- **Linear-time regex only**: Rust `regex` rejects lookahead, lookbehind, and backreferences.
