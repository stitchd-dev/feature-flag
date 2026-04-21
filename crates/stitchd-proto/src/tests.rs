//! Compilation tests — verify that all generated proto types are accessible
//! and the crate builds cleanly with the expected module structure.

#[cfg(test)]
mod compilation_tests {
    use crate::{common, events, flags, segments};

    #[test]
    fn common_types_accessible() {
        let _: Option<common::v1::Context> = None;
        let _: Option<common::v1::ParameterValue> = None;
    }

    #[test]
    fn flag_sync_types_accessible() {
        let _: Option<flags::v1::FeatureFlag> = None;
        let _: Option<flags::v1::SyncRequest> = None;
        let _: Option<flags::v1::SyncResponse> = None;
        let _: Option<flags::v1::Variant> = None;
        let _: Option<flags::v1::FlagRule> = None;
    }

    #[test]
    fn segment_types_accessible() {
        let _: Option<segments::v1::RuleSegment> = None;
        let _: Option<segments::v1::ListSegment> = None;
        let _: Option<segments::v1::SegmentBundle> = None;
    }

    #[test]
    fn event_types_accessible() {
        let _: Option<events::v1::IngestRequest> = None;
        let _: Option<events::v1::IngestResponse> = None;
        let _: Option<events::v1::Event> = None;
    }
}
