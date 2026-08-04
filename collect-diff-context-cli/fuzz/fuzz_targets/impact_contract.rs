#![no_main]

use collect_diff_context_cli::impact_context::contracts::ImpactContext;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(context) = serde_json::from_slice::<ImpactContext>(data) else {
        return;
    };
    let first_validation = context.validate().map_err(|error| error.to_string());
    let serialized = serde_json::to_vec(&context).expect("typed impact context must serialize");
    let round_trip: ImpactContext =
        serde_json::from_slice(&serialized).expect("serialized impact context must deserialize");
    let second_validation = round_trip.validate().map_err(|error| error.to_string());

    assert_eq!(context, round_trip);
    assert_eq!(first_validation, second_validation);
});
