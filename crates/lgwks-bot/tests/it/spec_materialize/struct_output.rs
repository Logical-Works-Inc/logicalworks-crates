//! A source whose output is a struct.

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::domains;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::registry::{Action, Source};
use lgwks_bot::spec::{Admission, Bot, BotSpec, Need};
use lgwks_bot::verb::{EffectLifetime, Execute, Observe};

use super::{TestResult, record, scope, tick_and_trace};

/// A structured observation: not ordered and not parseable from text, which is
/// the common shape of a real domain's state (`sys::ProcessState`, a PR status).
#[derive(Clone, Debug, PartialEq)]
struct Reading {
    /// What was read.
    label: String,
}

impl lgwks_bot::effect::InputIdentity for Reading {
    const SCHEMA_ID: &'static [u8] = b"lgwks.bot.schema.v1.test.reading";

    fn write_identity(&self, hasher: &mut lgwks_std::hash::Hasher) {
        hasher.write_framed(self.label.as_bytes());
    }
}

/// A source producing [`Reading`]s, registered through plain `Source::new`.
struct Reader(String);

impl Reader {
    /// Build one from the `target` its spec names.
    fn from_target(target: &str) -> Source {
        Source::new(Self(target.to_owned()))
    }
}

impl Observe for Reader {
    type Output = Reading;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<Reading, EcsError> {
        call.0.check(&[])?;
        record(format!("poll:{}", self.domain_id()));
        Ok(Reading {
            label: self.0.clone(),
        })
    }

    fn domain_id(&self) -> &str {
        "test::reader"
    }
}

/// An action that takes a [`Reading`].
struct Note;

impl Note {
    /// Build one from any `target`.
    fn from_target(_target: &str) -> Action {
        Action::new(Self)
    }
}

impl Execute for Note {
    type Input = Reading;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &Reading)) -> Result<(), EcsError> {
        call.0.check(&[])?;
        record(format!("execute:{}:{}", self.domain_id(), call.1.label));
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::note"
    }
}

/// The registry's constructor for [`Reader`], which cannot fail.
const READER: fn(&str) -> Result<Source, EcsError> = |target| Ok(Reader::from_target(target));

/// The registry's constructor for [`Note`], which cannot fail.
const NOTE: fn(&str) -> Result<Action, EcsError> = |target| Ok(Note::from_target(target));

domains! {
    /// A registry holding a struct-output source.
    pub STRUCT_DOMAINS {
        observe { "test::reader" => READER, }
        execute { "test::note" => NOTE, }
    }
}

/// Materialize against [`STRUCT_DOMAINS`].
fn admit_struct(json: &str) -> TestResult<Result<Bot, Admission>> {
    let spec = BotSpec::from_json(json)?;
    Ok(Bot::from_spec(
        &spec,
        &STRUCT_DOMAINS,
        &GrantSet::empty(),
        scope()?,
    ))
}

#[test]
fn a_struct_output_source_registers_and_answers_the_order_free_conditions() -> TestResult<()> {
    let mut bot = admit_struct(
        r#"{"version":1,"name":"struct","chains":[
            {"source":"test::reader","target":"r",
             "on":[["changed",{"domain":"test::note","target":""}]]}]}"#,
    )??;
    let first = tick_and_trace(&mut bot)?;
    let second = tick_and_trace(&mut bot)?;
    assert_eq!(
        first,
        (
            1,
            vec![
                "poll:test::reader".to_owned(),
                "execute:test::note:r".to_owned()
            ]
        ),
        "a struct output needs no ordering to be observed and to fire on change"
    );
    assert_eq!(second.0, 0, "an unchanged struct does not fire again");
    Ok(())
}

#[test]
fn a_threshold_on_an_unordered_source_is_an_unknown_condition_need() -> TestResult<()> {
    let refusal = admit_struct(
        r#"{"version":1,"name":"struct","chains":[
            {"source":"test::reader","target":"r",
             "on":[["threshold::above(1)",{"domain":"test::note","target":""}]]}]}"#,
    )?;
    let Err(Admission::Needs(needs)) = refusal else {
        return Err(format!("expected a NeedSet, got {refusal:?}").into());
    };
    assert_eq!(
        needs.needs(),
        [Need::UnknownCondition {
            chain: 0,
            action: 0,
            condition: "threshold::above(1)".into(),
        }],
        "a source that is not ordered has no threshold vocabulary"
    );
    Ok(())
}
