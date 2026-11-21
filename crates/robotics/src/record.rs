use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// An event timeline record indexed by wallclock time
/// (draft-ietf-moq-msf-02 §8.2): `{"T": .., "data": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record<D> {
    /// Milliseconds since the Unix epoch on the sender's clock.
    #[serde(rename = "T")]
    pub t: u64,
    pub data: D,
}

/// The payload of an event timeline object holding `record` alone: a JSON
/// array of one record (draft-ietf-moq-msf-02 §8.2). Fails only when `D`'s
/// `Serialize` fails.
pub fn encode_object<D: Serialize>(record: &Record<D>) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(std::slice::from_ref(record))?)
}

/// The records of an event timeline object payload. Fails when it is not a
/// JSON array of `T`-indexed records whose `data` is a `D`.
pub fn decode_object<D: DeserializeOwned>(payload: &[u8]) -> anyhow::Result<Vec<Record<D>>> {
    Ok(serde_json::from_slice(payload)?)
}

/// A `state` record's data: `seq` beside the robot's own fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub seq: u64,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// An `action` record's data: a chunk of actions answering the observation
/// at robot time `basis` (milliseconds on the robot's clock).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub seq: u64,
    pub basis: u64,
    pub dt_ms: u64,
    pub actions: Vec<Vec<f64>>,
}

impl Action {
    /// The robot time entry `index` of `actions` is meant for,
    /// `basis + (index + 1) × dt_ms`, or `None` when it exceeds `u64::MAX`.
    /// It does not check that `index` is within `actions`.
    pub fn entry_time(&self, index: usize) -> Option<u64> {
        let offset = u64::try_from(index)
            .ok()?
            .checked_mul(self.dt_ms)?
            .checked_add(self.dt_ms)?;
        self.basis.checked_add(offset)
    }
}

/// A `control` record's data: who drives the robot and why it changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Control {
    /// The driver's namespace relative to the room, `None` when nobody
    /// drives.
    pub driver: Option<String>,
    pub reason: ControlReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlReason {
    /// An operator's fresh action took the robot.
    Action,
    Left,
    Stale,
    Episode,
}

impl ControlReason {
    /// The value as the record writes it, for the `control_reason` log
    /// field.
    pub fn name(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Left => "left",
            Self::Stale => "stale",
            Self::Episode => "episode",
        }
    }
}

/// An `episode` record's data: the first object of an episode's group
/// starts it, the last ends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Episode {
    Start { id: u64, task: String },
    End { id: u64, success: bool },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn encode_str<D: Serialize>(t: u64, data: D) -> String {
        String::from_utf8(encode_object(&Record { t, data }).unwrap()).unwrap()
    }

    #[test]
    fn state_puts_the_robot_fields_beside_seq() {
        // Arrange
        let mut fields = Map::new();
        fields.insert("gripper".to_string(), json!(0.8));
        fields.insert("joints".to_string(), json!([0.12, -0.4]));
        let state = State { seq: 4512, fields };

        // Act
        let encoded = encode_str(1763600000123, state.clone());

        // Assert
        assert_eq!(
            encoded,
            r#"[{"T":1763600000123,"data":{"seq":4512,"gripper":0.8,"joints":[0.12,-0.4]}}]"#
        );
        assert_eq!(
            decode_object::<State>(encoded.as_bytes()).unwrap(),
            [Record {
                t: 1763600000123,
                data: state
            }]
        );
    }

    #[test]
    fn action_serializes_as_the_design_shows() {
        // Arrange
        let action = Action {
            seq: 77,
            basis: 1763600000123,
            dt_ms: 20,
            actions: vec![vec![0.13, -0.41, 0.8]],
        };

        // Act / Assert
        assert_eq!(
            encode_str(1763600000180, action),
            r#"[{"T":1763600000180,"data":{"seq":77,"basis":1763600000123,"dt_ms":20,"actions":[[0.13,-0.41,0.8]]}}]"#
        );
    }

    #[test]
    fn control_serializes_driver_and_lowercase_reason() {
        // Arrange
        let control = Control {
            driver: Some("operators/alice".to_string()),
            reason: ControlReason::Action,
        };

        // Act / Assert
        assert_eq!(
            encode_str(5, control),
            r#"[{"T":5,"data":{"driver":"operators/alice","reason":"action"}}]"#
        );
    }

    #[test]
    fn control_without_driver_serializes_null() {
        // Arrange
        let control = Control {
            driver: None,
            reason: ControlReason::Stale,
        };

        // Act / Assert
        assert_eq!(
            encode_str(5, control),
            r#"[{"T":5,"data":{"driver":null,"reason":"stale"}}]"#
        );
    }

    #[test]
    fn control_reasons_are_lowercase_strings() {
        // Arrange
        let reasons = [
            ControlReason::Action,
            ControlReason::Left,
            ControlReason::Stale,
            ControlReason::Episode,
        ];

        // Act
        let names: Vec<String> = reasons
            .iter()
            .map(|reason| serde_json::to_string(reason).unwrap())
            .collect();

        // Assert
        assert_eq!(
            names,
            [r#""action""#, r#""left""#, r#""stale""#, r#""episode""#]
        );
    }

    #[test]
    fn episode_is_tagged_by_event() {
        // Arrange
        let start = Episode::Start {
            id: 12,
            task: "put the cup on the tray".to_string(),
        };
        let end = Episode::End {
            id: 12,
            success: true,
        };

        // Act / Assert
        assert_eq!(
            (encode_str(1, start), encode_str(2, end)),
            (
                r#"[{"T":1,"data":{"event":"start","id":12,"task":"put the cup on the tray"}}]"#
                    .to_string(),
                r#"[{"T":2,"data":{"event":"end","id":12,"success":true}}]"#.to_string()
            )
        );
    }

    #[test]
    fn decode_reads_every_record_of_an_object() {
        // Arrange
        let payload = br#"[{"T":1,"data":{"event":"start","id":1,"task":"pick"}},{"T":2,"data":{"event":"end","id":1,"success":false}}]"#;

        // Act
        let records = decode_object::<Episode>(payload).unwrap();

        // Assert
        assert_eq!(
            records,
            [
                Record {
                    t: 1,
                    data: Episode::Start {
                        id: 1,
                        task: "pick".to_string()
                    }
                },
                Record {
                    t: 2,
                    data: Episode::End {
                        id: 1,
                        success: false
                    }
                }
            ]
        );
    }

    #[test]
    fn decode_rejects_a_record_without_wallclock_index() {
        // Arrange
        let payload = br#"[{"L":[0,0],"data":{"event":"end","id":1,"success":true}}]"#;

        // Act / Assert
        assert!(decode_object::<Episode>(payload).is_err());
    }

    mod properties {
        use proptest::collection::{btree_map, vec};
        use proptest::prelude::*;
        use serde_json::Value;

        use super::*;

        fn edge_u64() -> impl Strategy<Value = u64> {
            prop_oneof![
                Just(0),
                Just(1),
                Just(u64::MAX),
                (0..=1024u64).prop_map(|below| u64::MAX - below),
                (0..=1024u64).prop_map(|below| (u64::MAX >> 1) - below),
                any::<u64>(),
            ]
        }

        fn finite_f64() -> impl Strategy<Value = f64> {
            prop_oneof![
                Just(0.0),
                Just(-1.5),
                Just(f64::MAX),
                any::<i32>().prop_map(f64::from),
                -1e6..1e6f64
            ]
        }

        fn json_value() -> impl Strategy<Value = Value> {
            prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::from),
                edge_u64().prop_map(Value::from),
                any::<i64>().prop_map(Value::from),
                ".{0,8}".prop_map(Value::from),
                vec(any::<i32>(), 0..4).prop_map(Value::from),
            ]
        }

        fn state() -> impl Strategy<Value = State> {
            (
                edge_u64(),
                btree_map("[a-z]{1,6}", json_value(), 0..5).prop_map(|fields| {
                    fields
                        .into_iter()
                        .filter(|(name, _)| name != "seq")
                        .collect::<Map<String, Value>>()
                }),
            )
                .prop_map(|(seq, fields)| State { seq, fields })
        }

        fn action() -> impl Strategy<Value = Action> {
            (
                edge_u64(),
                edge_u64(),
                edge_u64(),
                vec(vec(finite_f64(), 0..8), 0..4),
            )
                .prop_map(|(seq, basis, dt_ms, actions)| Action {
                    seq,
                    basis,
                    dt_ms,
                    actions,
                })
        }

        fn control() -> impl Strategy<Value = Control> {
            (
                proptest::option::of(".{0,12}"),
                prop_oneof![
                    Just(ControlReason::Action),
                    Just(ControlReason::Left),
                    Just(ControlReason::Stale),
                    Just(ControlReason::Episode),
                ],
            )
                .prop_map(|(driver, reason)| Control { driver, reason })
        }

        fn episode() -> impl Strategy<Value = Episode> {
            prop_oneof![
                (edge_u64(), ".{0,12}").prop_map(|(id, task)| Episode::Start { id, task }),
                (edge_u64(), any::<bool>()).prop_map(|(id, success)| Episode::End { id, success }),
            ]
        }

        fn round_trip<D>(t: u64, data: D) -> Result<(), TestCaseError>
        where
            D: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug + Clone,
        {
            // Arrange
            let record = Record { t, data };

            // Act
            let decoded = decode_object::<D>(&encode_object(&record).unwrap()).unwrap();

            // Assert
            prop_assert_eq!(decoded, vec![record]);
            Ok(())
        }

        proptest! {
            #[test]
            fn a_state_record_reads_back(t in edge_u64(), data in state()) {
                round_trip(t, data)?;
            }

            #[test]
            fn an_action_record_reads_back(t in edge_u64(), data in action()) {
                round_trip(t, data)?;
            }

            #[test]
            fn a_control_record_reads_back(t in edge_u64(), data in control()) {
                round_trip(t, data)?;
            }

            #[test]
            fn an_episode_record_reads_back(t in edge_u64(), data in episode()) {
                round_trip(t, data)?;
            }

            #[test]
            fn entry_time_equals_the_u128_model(
                basis in edge_u64(),
                dt_ms in edge_u64(),
                index in prop_oneof![
                    Just(0usize),
                    Just(usize::MAX),
                    (0..=16usize).prop_map(|below| usize::MAX - below),
                    0..64usize,
                    any::<usize>(),
                ],
            ) {
                // Arrange
                let action = Action { seq: 0, basis, dt_ms, actions: Vec::new() };
                let model = u128::from(basis) + (index as u128 + 1) * u128::from(dt_ms);

                // Act
                let time = action.entry_time(index);

                // Assert
                prop_assert_eq!(time, u64::try_from(model).ok());
            }
        }
    }
}
