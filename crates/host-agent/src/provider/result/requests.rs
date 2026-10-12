//! SEP-2322 input requests: standard Go JSON outside SDK content unions.
use super::{content, put, put_nonempty, required_string, CONTENT, ICON};
use crate::goerr::quote;
use crate::gojson::{self, Node, Value as G};
use crate::mcpsdk::{self, StructDef, Ty, TypeError};
use serde_json::{Map, Value};

static RAW: StructDef = StructDef {
    name: "mcp.raw",
    fields: &[("method", Ty::Str("string")), ("params", Ty::Raw)],
};
static ELICIT: StructDef = StructDef {
    name: "mcp.ElicitParams",
    fields: &[
        ("_meta", Ty::MapAny("mcp.Meta")),
        ("mode", Ty::Str("string")),
        ("message", Ty::Str("string")),
        ("requestedSchema", Ty::Raw),
        ("url", Ty::Str("string")),
        ("elicitationId", Ty::Str("string")),
    ],
};
static ROOTS: StructDef = StructDef {
    name: "mcp.ListRootsParams",
    fields: &[("_meta", Ty::MapAny("mcp.Meta"))],
};
static HINT: StructDef = StructDef {
    name: "mcp.ModelHint",
    fields: &[("name", Ty::Str("string"))],
};
static PREFERENCES: StructDef = StructDef {
    name: "mcp.ModelPreferences",
    fields: &[
        ("costPriority", Ty::Float),
        ("hints", Ty::PointerSlice(&HINT)),
        ("intelligencePriority", Ty::Float),
        ("speedPriority", Ty::Float),
    ],
};
static CHOICE: StructDef = StructDef {
    name: "mcp.ToolChoice",
    fields: &[("mode", Ty::Str("string"))],
};
static TOOL_ANNOTATIONS: StructDef = StructDef {
    name: "mcp.ToolAnnotations",
    fields: &[
        ("destructiveHint", Ty::PointerBool),
        ("idempotentHint", Ty::Bool),
        ("openWorldHint", Ty::PointerBool),
        ("readOnlyHint", Ty::Bool),
        ("title", Ty::Str("string")),
    ],
};
static TOOL: StructDef = StructDef {
    name: "mcp.Tool",
    fields: &[
        ("_meta", Ty::MapAny("mcp.Meta")),
        ("annotations", Ty::Struct(&TOOL_ANNOTATIONS)),
        ("description", Ty::Str("string")),
        ("inputSchema", Ty::Raw),
        ("name", Ty::Str("string")),
        ("outputSchema", Ty::Raw),
        ("title", Ty::Str("string")),
        ("icons", Ty::SliceOf(&ICON)),
    ],
};
static SAMPLING: StructDef = StructDef {
    name: "mcp.CreateMessageWithToolsParams",
    fields: &[
        ("_meta", Ty::MapAny("mcp.Meta")),
        ("includeContext", Ty::Str("string")),
        ("maxTokens", Ty::Int64),
        ("messages", Ty::Custom(validate_messages)),
        ("metadata", Ty::Raw),
        ("modelPreferences", Ty::Struct(&PREFERENCES)),
        ("stopSequences", Ty::SliceStr),
        ("systemPrompt", Ty::Str("string")),
        ("temperature", Ty::Float),
        ("tools", Ty::PointerSlice(&TOOL)),
        ("toolChoice", Ty::Struct(&CHOICE)),
    ],
};
static MESSAGE: StructDef = StructDef {
    name: r#"struct { mcp.msg; Content json.RawMessage "json:\"content\"" }"#,
    fields: &[("role", Ty::Str("string")), ("content", Ty::Raw)],
};

fn value(node: &Node) -> Value {
    serde_json::from_slice(&node.raw).expect("canonical JSON node")
}
fn kind(node: &Node, ty: &str) -> String {
    match &node.value {
        G::Bool(_) => "bool".into(),
        G::String(_) => "string".into(),
        G::Number(n) if matches!(ty, "int64" | "float64") => format!("number {n}"),
        G::Number(_) => "number".into(),
        G::Array(_) => "array".into(),
        G::Object(_) => "object".into(),
        G::Null => "null".into(),
    }
}
fn wrong(node: &Node, ty: &str, owner: &str, path: &str) -> TypeError {
    TypeError {
        raw: None,
        value: kind(node, ty),
        ty: ty.into(),
        strukt: if path.is_empty() {
            String::new()
        } else {
            owner.rsplit('.').next().unwrap_or(owner).into()
        },
        field: path.into(),
    }
}

#[derive(Default)]
struct StdState {
    slices: std::collections::BTreeMap<String, Vec<Value>>,
}
fn std_struct(node: &Node, def: &StructDef, path: &str) -> Result<Map<String, Value>, TypeError> {
    std_struct_into(node, def, path, "$", Map::new(), &mut StdState::default())
}
fn std_struct_into(
    node: &Node,
    def: &StructDef,
    path: &str,
    location: &str,
    mut out: Map<String, Value>,
    state: &mut StdState,
) -> Result<Map<String, Value>, TypeError> {
    if node.is_null() {
        return Ok(out);
    }
    let fields = node
        .as_object()
        .ok_or_else(|| wrong(node, def.name, def.name, path))?;
    let mut first = None;
    for (key, node) in fields {
        let Some((field, ty)) = def
            .fields
            .iter()
            .find(|(f, _)| *f == key)
            .or_else(|| def.fields.iter().find(|(f, _)| gojson::equal_fold(f, key)))
        else {
            continue;
        };
        let subpath = if path.is_empty() {
            field.to_string()
        } else {
            format!("{path}.{field}")
        };
        let previous = out.remove(*field).unwrap_or(Value::Null);
        match std_field(
            node,
            ty,
            def.name,
            &subpath,
            &format!("{location}.{field}"),
            previous,
            state,
        ) {
            Ok(v) => {
                out.insert(field.to_string(), v);
            }
            Err(e) => {
                if matches!(ty, Ty::Custom(_)) {
                    return Err(e);
                }
                if first.is_none() {
                    first = Some(e);
                }
            }
        }
    }
    if let Some(e) = first {
        Err(e)
    } else {
        Ok(out)
    }
}
fn std_field(
    node: &Node,
    ty: &Ty,
    owner: &str,
    path: &str,
    location: &str,
    previous: Value,
    state: &mut StdState,
) -> Result<Value, TypeError> {
    let error = |ty| wrong(node, ty, owner, path);
    if node.is_null() {
        return Ok(match ty {
            Ty::Str(_) if previous.is_null() => Value::String(String::new()),
            Ty::Bool if previous.is_null() => Value::Bool(false),
            Ty::Int64 if previous.is_null() => Value::from(0),
            Ty::Float if previous.is_null() => Value::from(0.0),
            Ty::Str(_) | Ty::Bool | Ty::Int64 | Ty::Float => previous,
            _ => {
                state
                    .slices
                    .retain(|key, _| key != location && !key.starts_with(&format!("{location}.")));
                Value::Null
            }
        });
    }
    match ty {
        Ty::Str(name) => node
            .as_str()
            .map(|v| Value::String(v.into()))
            .ok_or_else(|| error(name)),
        Ty::Bool | Ty::PointerBool => match node.value {
            G::Bool(b) => Ok(Value::Bool(b)),
            _ => Err(error("bool")),
        },
        Ty::Int64 => match &node.value {
            G::Number(n) => n
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| error("int64")),
            _ => Err(error("int64")),
        },
        Ty::Float => match &node.value {
            G::Number(n) => Ok(gojson::float_value(
                n.parse().expect("finite canonical number"),
            )),
            _ => Err(error("float64")),
        },
        Ty::Raw => Ok(value(node)),
        Ty::MapAny(name) => {
            let Some(members) = node.as_object() else {
                return Err(error(name));
            };
            let mut map = previous.as_object().cloned().unwrap_or_default();
            for (key, node) in members {
                map.insert(key.clone(), value(node));
            }
            Ok(Value::Object(map))
        }
        Ty::Struct(def) => {
            if node.as_object().is_none() {
                return Err(error(def.name));
            }
            std_struct_into(
                node,
                def,
                path,
                location,
                previous.as_object().cloned().unwrap_or_default(),
                state,
            )
            .map(Value::Object)
        }
        Ty::SliceStr | Ty::SliceOf(_) | Ty::PointerSlice(_) => {
            let typename = match ty {
                Ty::SliceStr => "[]string".into(),
                Ty::SliceOf(d) => format!("[]{}", d.name),
                Ty::PointerSlice(d) => format!("[]*{}", d.name),
                _ => unreachable!(),
            };
            let G::Array(items) = &node.value else {
                return Err(error(&typename));
            };
            let mut slots = state
                .slices
                .remove(location)
                .unwrap_or_else(|| previous.as_array().cloned().unwrap_or_default());
            for (i, n) in items.iter().enumerate() {
                if i == slots.len() {
                    slots.push(Value::Null);
                }
                let old = slots[i].clone();
                let item_location = format!("{location}.{i}");
                slots[i] = match ty {
                    Ty::SliceStr => std_field(
                        n,
                        &Ty::Str("string"),
                        owner,
                        path,
                        &item_location,
                        old,
                        state,
                    )?,
                    Ty::PointerSlice(_) if n.is_null() => {
                        state
                            .slices
                            .retain(|key, _| !key.starts_with(&format!("{item_location}.")));
                        Value::Null
                    }
                    Ty::SliceOf(def) | Ty::PointerSlice(def) => {
                        if !n.is_null() && n.as_object().is_none() {
                            return Err(wrong(n, def.name, owner, path));
                        }
                        Value::Object(std_struct_into(
                            n,
                            def,
                            path,
                            &item_location,
                            old.as_object().cloned().unwrap_or_default(),
                            state,
                        )?)
                    }
                    _ => unreachable!(),
                };
            }
            let result = slots[..items.len()].to_vec();
            state.slices.insert(location.into(), slots);
            Ok(Value::Array(result))
        }
        Ty::Custom(validate) => {
            validate(node, &node.raw).map_err(|mut e| {
                if e.raw.is_none() {
                    e.strukt = owner.rsplit('.').next().unwrap_or(owner).into();
                    e.field = if e.field.is_empty() {
                        path.into()
                    } else {
                        format!("{path}.{}", e.field)
                    };
                }
                e
            })?;
            Ok(value(node))
        }
        _ => unreachable!("input-request field table"),
    }
}

fn validate_messages(node: &Node, _: &[u8]) -> Result<(), TypeError> {
    let G::Array(items) = &node.value else {
        return Err(wrong(node, "[]*mcp.SamplingMessageV2", "", ""));
    };
    for item in items {
        if !item.is_null() {
            sampling_message(item)?;
        }
    }
    Ok(())
}
fn sampling_message(node: &Node) -> Result<Value, TypeError> {
    let local = gojson::parse(&node.raw).expect("canonical sampling message");
    mcpsdk::validate_type_exact_error(&local, &Ty::Struct(&MESSAGE), &node.raw)?;
    let Some(raw) = local.key("content").filter(|n| !n.is_null()) else {
        return Err("nil content".into());
    };
    let content_node = gojson::parse(&raw.raw).expect("canonical content");
    let items = match &content_node.value {
        G::Array(items)
            if mcpsdk::validate_type_exact_error(
                &content_node,
                &Ty::PointerSlice(&CONTENT),
                &raw.raw,
            )
            .is_ok() =>
        {
            items.clone()
        }
        _ => {
            mcpsdk::validate_type_exact_error(&content_node, &Ty::Struct(&CONTENT), &raw.raw)?;
            vec![content_node]
        }
    };
    let mut projected = Vec::new();
    for item in items {
        if item.is_null() {
            return Err("nil content".into());
        }
        let kind = item.key("type").and_then(Node::as_str).unwrap_or_default();
        if !matches!(
            kind,
            "text" | "image" | "audio" | "tool_use" | "tool_result"
        ) {
            return Err(format!("invalid content type {}", quote(kind)).into());
        }
        projected.push(content(&value(&item), false)?);
    }
    let mut out = Map::new();
    out.insert(
        "role".into(),
        Value::String(
            local
                .key("role")
                .and_then(Node::as_str)
                .unwrap_or_default()
                .into(),
        ),
    );
    out.insert(
        "content".into(),
        if projected.len() == 1 {
            projected.remove(0)
        } else {
            Value::Array(projected)
        },
    );
    Ok(Value::Object(out))
}
fn omit_fields(source: &Map<String, Value>, fields: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    for key in fields {
        put_nonempty(&mut out, source, key);
    }
    out
}
fn tool(source: &Value) -> Value {
    if source.is_null() {
        return Value::Null;
    }
    let source = source.as_object().expect("typed tool");
    let mut out = omit_fields(source, &["_meta", "description", "title"]);
    put(&mut out, source, "outputSchema");
    required_string(&mut out, source, "name");
    out.insert(
        "inputSchema".into(),
        source.get("inputSchema").cloned().unwrap_or(Value::Null),
    );
    if let Some(Value::Object(a)) = source.get("annotations") {
        let mut annotations = omit_fields(a, &["title"]);
        for key in ["destructiveHint", "openWorldHint"] {
            put(&mut annotations, a, key);
        }
        for key in ["idempotentHint", "readOnlyHint"] {
            annotations.insert(
                key.into(),
                a.get(key).cloned().unwrap_or(Value::Bool(false)),
            );
        }
        out.insert("annotations".into(), Value::Object(annotations));
    }
    if let Some(Value::Array(icons)) = source
        .get("icons")
        .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
    {
        out.insert(
            "icons".into(),
            Value::Array(
                icons
                    .iter()
                    .map(|icon| {
                        let source = icon.as_object().expect("typed icon");
                        let mut icon = omit_fields(source, &["mimeType", "sizes", "theme"]);
                        required_string(&mut icon, source, "src");
                        Value::Object(icon)
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(out)
}
fn sampling(source: &Map<String, Value>) -> Result<Value, TypeError> {
    let mut out = omit_fields(
        source,
        &[
            "_meta",
            "includeContext",
            "metadata",
            "stopSequences",
            "systemPrompt",
            "temperature",
        ],
    );
    put(&mut out, source, "metadata");
    out.insert(
        "maxTokens".into(),
        source.get("maxTokens").cloned().unwrap_or(Value::from(0)),
    );
    out.insert(
        "messages".into(),
        match source.get("messages") {
            Some(Value::Array(items)) => Value::Array(
                items
                    .iter()
                    .map(|n| {
                        if n.is_null() {
                            Ok(Value::Null)
                        } else {
                            let mut encoded = String::new();
                            gojson::encode(n, &mut encoded);
                            sampling_message(
                                &gojson::parse(encoded.as_bytes()).expect("canonical message"),
                            )
                        }
                    })
                    .collect::<Result<_, _>>()?,
            ),
            _ => Value::Null,
        },
    );
    if let Some(Value::Object(p)) = source.get("modelPreferences") {
        let mut prefs = omit_fields(
            p,
            &["costPriority", "intelligencePriority", "speedPriority"],
        );
        if let Some(Value::Array(hints)) = p
            .get("hints")
            .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        {
            prefs.insert(
                "hints".into(),
                Value::Array(
                    hints
                        .iter()
                        .map(|h| {
                            if let Some(h) = h.as_object() {
                                Value::Object(omit_fields(h, &["name"]))
                            } else {
                                Value::Null
                            }
                        })
                        .collect(),
                ),
            );
        }
        out.insert("modelPreferences".into(), Value::Object(prefs));
    }
    if let Some(Value::Object(c)) = source.get("toolChoice") {
        out.insert(
            "toolChoice".into(),
            Value::Object(omit_fields(c, &["mode"])),
        );
    }
    if let Some(Value::Array(tools)) = source
        .get("tools")
        .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
    {
        out.insert(
            "tools".into(),
            Value::Array(tools.iter().map(tool).collect()),
        );
    }
    Ok(Value::Object(out))
}

pub(super) fn validate(node: &Node, _: &[u8]) -> Result<(), TypeError> {
    project_typed(node).map(|_| ())
}
pub(super) fn project(node: &Node) -> Result<Value, String> {
    project_typed(node).map_err(|e| e.message())
}
fn project_typed(node: &Node) -> Result<Value, TypeError> {
    if node.is_null() {
        return Ok(Value::Null);
    }
    let entries = node
        .as_object()
        .ok_or_else(|| wrong(node, "map[string]*mcp.raw", "", ""))?;
    let mut out = Map::new();
    // Decode the complete raw map before method dispatch, as encoding/json does.
    let raw = entries
        .iter()
        .map(|(id, n)| std_struct(n, &RAW, "").map(|r| (id, n, r)))
        .collect::<Result<Vec<_>, _>>()?;
    // Refuse the proposed null-entry difference before dispatching any method.
    // Raw-map type errors retain their existing decoding precedence.
    if raw.iter().any(|(_, node, _)| node.is_null()) {
        return Err("nil InputRequest entry".into());
    }
    for (id, node, request) in raw {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let def = match method {
            "elicitation/create" => &ELICIT,
            "sampling/createMessage" => &SAMPLING,
            "roots/list" => &ROOTS,
            _ => return Err(format!("unsupported InputRequest method: {}", quote(method)).into()),
        };
        let fields = node.as_object().expect("raw request object");
        let params = fields
            .iter()
            .filter(|(key, _)| gojson::equal_fold(key, "params"))
            .map(|(_, n)| n)
            .next_back()
            .ok_or_else(|| TypeError::from("unexpected end of JSON input"))?;
        let source = std_struct(params, def, "")?;
        let params = match method {
            "elicitation/create" => {
                let mut p = omit_fields(&source, &["_meta", "url", "elicitationId"]);
                put(&mut p, &source, "requestedSchema");
                required_string(&mut p, &source, "message");
                let mode = source
                    .get("mode")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if ["url", "elicitationId"].iter().any(|k| {
                            source
                                .get(*k)
                                .and_then(Value::as_str)
                                .is_some_and(|v| !v.is_empty())
                        }) {
                            "url"
                        } else {
                            "form"
                        }
                    });
                p.insert("mode".into(), Value::String(mode.into()));
                Value::Object(p)
            }
            "roots/list" => Value::Object(omit_fields(&source, &["_meta"])),
            _ => sampling(&source)?,
        };
        out.insert(
            id.clone(),
            serde_json::json!({"method":method,"params":params}),
        );
    }
    Ok(Value::Object(out))
}
