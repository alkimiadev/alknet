use serde_json::Value;

use super::context::{AbortPolicy, OperationContext};
use crate::protocol::wire::ResponseEnvelope;

#[async_trait::async_trait]
pub trait OperationEnv: Send + Sync {
    async fn invoke(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
    ) -> ResponseEnvelope {
        self.invoke_with_policy(namespace, operation, input, parent, parent.abort_policy)
            .await
    }

    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope;

    fn contains(&self, name: &str) -> bool {
        let _ = name;
        true
    }
}
