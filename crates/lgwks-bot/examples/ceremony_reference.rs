//! Ceremony reference app: request, model draft, approval, send.
use lgwks_bot::Cap;
use lgwks_bot::app::{App, Effect, Method, Provider, ReplyTag};
use std::error::Error;

/// The model and notifier stand-in behind the reference.
struct DraftSend;
impl Provider for DraftSend {
    /// Answer asks with the tenant and prompt size, sends as queued.
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>> {
        match *effect {
            Effect::Ask { ref prompt, .. } => {
                Some([tenant.as_bytes(), prompt.len().to_string().as_bytes()].concat())
            }
            Effect::Send { .. } => Some(b"queued".to_vec()),
            _ => None,
        }
    }
}
/// Build the reference app, check it, and report readiness.
fn main() -> Result<(), Box<dyn Error>> {
    let app = App::new("ceremony")
        .grant(Cap::new("app.ask"))
        .grant(Cap::notify())
        .grant(Cap::new("app.approval"))
        .provider(DraftSend)
        .route(Method::POST, "/request", |_: &mut (), body: String| {
            vec![
                Effect::reply(201, b"parked".to_vec()),
                Effect::Ask {
                    prompt: body.into(),
                    reply_to: ReplyTag::new("drafted"),
                },
            ]
        })
        .on(ReplyTag::new("drafted"), |_: &mut (), input| {
            match *input.result() {
                lgwks_bot::app::EffectResult::Value(ref bytes) => {
                    let text = String::from_utf8_lossy(bytes).into_owned();
                    vec![Effect::AwaitApproval {
                        what: text.into(),
                        reply_to: ReplyTag::new("approved"),
                    }]
                }
                _ => vec![Effect::reply(502, b"no draft".to_vec())],
            }
        })
        .on(ReplyTag::new("approved"), |_: &mut (), _| {
            vec![Effect::Send {
                to: "ops".into(),
                body: b"sealed".to_vec().into(),
                reply_to: ReplyTag::new("sent"),
            }]
        });
    app.check()?;
    Ok(())
}
