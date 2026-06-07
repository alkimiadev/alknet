pub const CALL_REQUESTED: &str = "call.requested";
pub const CALL_RESPONDED: &str = "call.responded";
pub const CALL_COMPLETED: &str = "call.completed";
pub const CALL_ABORTED: &str = "call.aborted";
pub const CALL_ERROR: &str = "call.error";

pub const SERVICE_LIST: &str = "/services/list";
pub const SERVICE_SCHEMA: &str = "/services/schema";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_constants() {
        assert_eq!(CALL_REQUESTED, "call.requested");
        assert_eq!(CALL_RESPONDED, "call.responded");
        assert_eq!(CALL_COMPLETED, "call.completed");
        assert_eq!(CALL_ABORTED, "call.aborted");
        assert_eq!(CALL_ERROR, "call.error");
    }

    #[test]
    fn service_operation_constants() {
        assert_eq!(SERVICE_LIST, "/services/list");
        assert_eq!(SERVICE_SCHEMA, "/services/schema");
    }
}
