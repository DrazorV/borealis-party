//! The bring-your-own model as the simulation's assistant oracle (part C's provider, 0.3.1).
//!
//! main.rs calls [`install`] at start-up. Only models the
//! server may call (hosted ones through `net_guard`) play here; a local or LAN model ("device" mode) has no
//! server-side model, so `sim_auto {oracle: "assistant"}` answers "unavailable" for it.

use std::sync::Arc;
use std::time::Duration;

use party_sim::oracle::Profile;
use serde_json::Value;

use crate::ai_assistant::{self, AccountModel};
use crate::auth::User;
use crate::simulate::{self, Decider, DeciderFuture};
use crate::AppState;

/// Install the account model source into the simulator.
pub fn install() {
    simulate::install_decider(source);
}

fn source(state: AppState, user: User) -> DeciderFuture {
    Box::pin(async move {
        let model = ai_assistant::model_of(&state, &user).await.ok().flatten()?;
        let rt = tokio::runtime::Handle::current();
        Some(Arc::new(Hosted {
            state,
            user,
            model,
            rt,
        }) as Arc<dyn Decider>)
    })
}

struct Hosted {
    state: AppState,
    user: User,
    model: AccountModel,
    rt: tokio::runtime::Handle,
}

impl Decider for Hosted {
    fn decide(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.rt.block_on(async {
            ai_assistant::admit_call(&self.state, &self.user)
                .await
                .map_err(|_| "the account's AI call limit was reached".to_string())?;
            let (value, usage) = crate::ai_providers::complete_json(
                &self.model.endpoint,
                system,
                user,
                schema,
                timeout,
            )
            .await
            .map_err(|e| e.message)?;
            let _ = ai_assistant::record_usage(
                &self.state,
                &self.user,
                usage.input_tokens + usage.output_tokens,
            )
            .await;
            Ok(value)
        })
    }

    fn model(&self) -> String {
        self.model.endpoint.model.clone()
    }

    fn is_local(&self) -> bool {
        false
    }

    fn profile(&self) -> Option<Profile> {
        Some(match self.model.profile {
            ai_assistant::Profile::Small => Profile::Small,
            ai_assistant::Profile::Frontier => Profile::Frontier,
        })
    }
}
