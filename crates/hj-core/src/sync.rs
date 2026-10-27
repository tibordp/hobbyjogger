//! Turning state changes into immer patches for the frontend.
//!
//! The state is serialized to JSON after every change and diffed against the previously published
//! JSON with `json-patch` (RFC 6902). The result is converted to immer's patch format: paths as
//! arrays, with array indices as numbers. `json-patch` only produces add/remove/replace with
//! numeric indices, applied in order, so each operation is resolved against a working copy that
//! has the previous operations applied.

use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum PatchOp {
    Add,
    Remove,
    Replace,
}

/// One immer patch.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Patch {
    pub op: PatchOp,
    pub path: Vec<PathSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub value: Option<Value>,
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Diff `before` → `after` as immer patches.
pub fn diff(before: &Value, after: &Value) -> Vec<Patch> {
    let ops = json_patch::diff(before, after);
    let mut working = before.clone();
    let mut out = Vec::with_capacity(ops.0.len());
    for op in ops.0 {
        // Serialize to get the plain RFC 6902 shape without depending on json-patch internals.
        let raw = serde_json::to_value(&op).expect("patch op serializes");
        let kind = match raw["op"].as_str() {
            Some("add") => PatchOp::Add,
            Some("remove") => PatchOp::Remove,
            Some("replace") => PatchOp::Replace,
            other => unreachable!("json-patch diff produced {other:?}"),
        };
        let pointer = raw["path"].as_str().unwrap_or_default();

        let mut path = Vec::new();
        let mut cursor = Some(&working);
        for raw_segment in pointer.split('/').skip(1) {
            let segment = unescape(raw_segment);
            let seg = match cursor {
                Some(Value::Array(items)) => {
                    let index = segment.parse::<usize>().unwrap_or(items.len());
                    cursor = items.get(index);
                    PathSegment::Index(index)
                }
                Some(Value::Object(map)) => {
                    cursor = map.get(&segment);
                    PathSegment::Key(segment)
                }
                _ => {
                    cursor = None;
                    PathSegment::Key(segment)
                }
            };
            path.push(seg);
        }

        json_patch::patch(&mut working, &[op]).expect("diff applies to its own source");
        out.push(Patch {
            op: kind,
            path,
            value: raw.get("value").cloned(),
        });
    }
    debug_assert_eq!(&working, after);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Apply immer-style patches (the frontend's job) to check the conversion round-trips.
    fn apply(doc: &mut Value, patches: &[Patch]) {
        for p in patches {
            let (last, parents) = p.path.split_last().unwrap();
            let mut target = &mut *doc;
            for seg in parents {
                target = match seg {
                    PathSegment::Key(k) => &mut target[k.as_str()],
                    PathSegment::Index(i) => &mut target[*i],
                };
            }
            match (p.op, last, target) {
                (PatchOp::Remove, PathSegment::Index(i), Value::Array(a)) => {
                    a.remove(*i);
                }
                (PatchOp::Add, PathSegment::Index(i), Value::Array(a)) => {
                    a.insert(*i, p.value.clone().unwrap())
                }
                (PatchOp::Remove, PathSegment::Key(k), Value::Object(m)) => {
                    m.remove(k);
                }
                (_, PathSegment::Key(k), Value::Object(m)) => {
                    m.insert(k.clone(), p.value.clone().unwrap());
                }
                (_, PathSegment::Index(i), Value::Array(a)) => a[*i] = p.value.clone().unwrap(),
                other => panic!("bad patch target {other:?}"),
            }
        }
    }

    #[test]
    fn round_trips_objects_and_arrays() {
        let before = json!({"a": 1, "list": [1, 2, 3, 4], "obj": {"x/y": true, "gone": 1}});
        let after = json!({"a": 2, "list": [1, 3], "obj": {"x/y": false, "new": [5]}, "b": null});
        let patches = diff(&before, &after);
        assert!(patches.iter().any(|p| p.path
            == vec![
                PathSegment::Key("obj".into()),
                PathSegment::Key("x/y".into())
            ]));
        assert!(
            patches
                .iter()
                .any(|p| matches!(p.path.last(), Some(PathSegment::Index(_))))
        );
        let mut doc = before.clone();
        apply(&mut doc, &patches);
        assert_eq!(doc, after);
    }

    #[test]
    fn growing_array_uses_indices() {
        let before = json!({"list": []});
        let after = json!({"list": [{"n": 1}, {"n": 2}]});
        let patches = diff(&before, &after);
        let mut doc = before.clone();
        apply(&mut doc, &patches);
        assert_eq!(doc, after);
    }

    #[test]
    fn no_change_no_patches() {
        let v = json!({"a": [1, {"b": 2}]});
        assert!(diff(&v, &v).is_empty());
    }
}
