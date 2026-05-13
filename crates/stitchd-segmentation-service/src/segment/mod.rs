//! Segment domain helpers — proto mapping utilities.

use stitchd_core::{id::EnvironmentId, rule_engine::types::Rule, segment::Segment};
use stitchd_proto::segments::v1::{ListSegment, ListSegmentMeta, RuleSegment};

use crate::error::ServiceError;

/// Convert a [`Segment`] record plus its definition into a [`RuleSegment`] proto message.
///
/// # Errors
/// Returns [`ServiceError::Internal`] if rule serialisation fails.
pub fn segment_to_rule_proto(seg: &Segment, rules: &[Rule]) -> Result<RuleSegment, ServiceError> {
    let rule_payload = serde_json::to_vec(rules)
        .map_err(|e| ServiceError::Internal(format!("rule serialisation: {e}")))?;

    Ok(RuleSegment {
        key: seg.key.clone(),
        context_type: String::new(),
        rule_payload,
        id: seg.id.to_string(),
    })
}

/// Convert a [`Segment`] record into a [`ListSegmentMeta`] proto message.
#[must_use]
pub fn segment_to_list_meta_proto(seg: &Segment) -> ListSegmentMeta {
    ListSegmentMeta {
        key: seg.key.clone(),
        context_type: String::new(),
        id: seg.id.to_string(),
    }
}

/// Convert a [`Segment`] record plus list definition into a [`ListSegment`] proto message.
#[must_use]
pub fn segment_to_list_proto(
    seg: &Segment,
    def: &stitchd_core::segment::ListBasedSegment,
) -> ListSegment {
    let (included_keys, excluded_keys) =
        def.lists
            .values()
            .fold((vec![], vec![]), |(mut inc, mut exc), ctx_list| {
                inc.extend(ctx_list.include.iter().cloned());
                exc.extend(ctx_list.exclude.iter().cloned());
                (inc, exc)
            });

    ListSegment {
        key: seg.key.clone(),
        context_type: String::new(),
        included_keys,
        excluded_keys,
    }
}

/// Parse an environment ID from a string.
///
/// # Errors
/// Returns [`ServiceError::InvalidArgument`] if the string is not a valid UUID.
pub fn parse_env_id(s: &str) -> Result<EnvironmentId, ServiceError> {
    s.parse::<uuid::Uuid>()
        .map(EnvironmentId::from_uuid)
        .map_err(|_| ServiceError::InvalidArgument(format!("invalid environment_id: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use stitchd_core::{
        id::{EnvironmentId, SegmentId},
        segment::{ContextList, ListBasedSegment, Segment, SegmentType},
    };

    fn make_segment(seg_type: SegmentType) -> Segment {
        Segment {
            id: SegmentId::new(),
            environment_id: EnvironmentId::new(),
            key: "test-seg".to_string(),
            name: String::new(),
            description: String::new(),
            tags: vec![],
            segment_type: seg_type,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    #[test]
    fn segment_to_rule_proto_serialises_rules() {
        let seg = make_segment(SegmentType::Rule);
        let rules: Vec<Rule> = vec![];
        let proto = segment_to_rule_proto(&seg, &rules).expect("serialisation should succeed");
        assert_eq!(proto.key, "test-seg");
        assert_eq!(proto.id, seg.id.to_string());
        // Empty rules serialise to "[]"
        assert_eq!(proto.rule_payload, b"[]");
    }

    #[test]
    fn segment_to_list_meta_proto_maps_correctly() {
        let seg = make_segment(SegmentType::List);
        let meta = segment_to_list_meta_proto(&seg);
        assert_eq!(meta.key, "test-seg");
        assert_eq!(meta.id, seg.id.to_string());
    }

    #[test]
    fn segment_to_list_proto_maps_keys() {
        let seg = make_segment(SegmentType::List);
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string(), "u2".to_string()]
                    .iter()
                    .cloned()
                    .collect(),
                exclude: std::iter::once("u3".to_string()).collect(),
            },
        );
        let def = ListBasedSegment { id: seg.id, lists };
        let proto = segment_to_list_proto(&seg, &def);
        assert_eq!(proto.key, "test-seg");
        assert!(proto.included_keys.contains(&"u1".to_string()));
        assert!(proto.included_keys.contains(&"u2".to_string()));
        assert!(proto.excluded_keys.contains(&"u3".to_string()));
    }

    #[test]
    fn parse_env_id_accepts_valid_uuid() {
        let id = uuid::Uuid::new_v4().to_string();
        assert!(parse_env_id(&id).is_ok());
    }

    #[test]
    fn parse_env_id_rejects_invalid_string() {
        let result = parse_env_id("not-a-uuid");
        assert!(matches!(result, Err(ServiceError::InvalidArgument(_))));
    }
}
