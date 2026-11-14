use super::*;

#[test]
fn a_failure_of_the_run_comes_before_those_of_the_trace_and_the_save() {
    let err = |text: &str| RunError::Fault(text.to_string());
    let mut said = Vec::new();
    let last = settle(
        Err(err("fault")),
        [Err(err("trace")), Err(err("shot")), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    // The last is returned for the caller to say; the others were said, in order, before it.
    assert_eq!(last.to_string(), "save");
    assert_eq!(
        String::from_utf8(said).unwrap(),
        "cmm2-emulator: fault\ncmm2-emulator: trace\ncmm2-emulator: shot\n"
    );

    // With no fault the order is the trace, the screenshot, the save.
    let mut said = Vec::new();
    let last = settle(
        Ok(true),
        [Err(err("trace")), Ok(()), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert_eq!(String::from_utf8(said).unwrap(), "cmm2-emulator: trace\n");

    // One error alone is only returned, and none says nothing.
    let mut said = Vec::new();
    let last = settle(Err(err("fault")), [Ok(()), Ok(()), Ok(())], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "fault");
    let last = settle(Ok(true), [Ok(()), Ok(()), Err(err("save"))], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert!(matches!(
        settle(Ok(false), [Ok(()), Ok(()), Ok(())], &mut said),
        Ok(false)
    ));
    assert!(said.is_empty());
}
