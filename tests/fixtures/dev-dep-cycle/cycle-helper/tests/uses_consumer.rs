#[test]
fn helper_can_use_consumer_as_dev_dependency() {
	assert_eq!(cycle_consumer::consumer_value(), 43);
}
