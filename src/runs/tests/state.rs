use super::*;

// -----------------------------------------------------------------
// RunState parsing
// -----------------------------------------------------------------

#[test]
fn run_state_round_trip_through_str() {
    for state in RunState::all() {
        let parsed: RunState = state.as_str().parse().unwrap();
        assert_eq!(parsed, *state);
    }
}

#[test]
fn run_state_invalid_string_returns_helpful_error() {
    let err = "paused".parse::<RunState>().unwrap_err();
    assert!(err.contains("invalid run state"));
    assert!(err.contains("queued"));
}

#[test]
fn state_set_in_flight_and_terminal() {
    let in_flight = RunStateSet::InFlight.to_states();
    assert!(in_flight.contains(&RunState::Queued));
    assert!(in_flight.contains(&RunState::Running));
    let terminal = RunStateSet::Terminal.to_states();
    assert!(terminal.contains(&RunState::Completed));
    assert!(terminal.contains(&RunState::DeadLetter));
    assert!(!terminal.contains(&RunState::Queued));
}

#[test]
fn run_state_display_matches_as_str() {
    for state in RunState::all() {
        assert_eq!(format!("{}", state), state.as_str());
    }
}

#[test]
fn run_state_serde_roundtrip() {
    for state in RunState::all() {
        let json = serde_json::to_string(state).unwrap();
        let parsed: RunState = serde_json::from_str(&json).unwrap();
        assert_eq!(*state, parsed);
    }
    // Invalid state string deserializes as error.
    let err: Result<RunState, _> = serde_json::from_str("\"bogus\"");
    assert!(err.is_err());
}
