use crate::run_executor::execute_with_heartbeat as heartbeat;

fn run_direct() {
    heartbeat();
    crate::run_executor::execute_with_heartbeat();
}

fn run_worker() {
    crate::run_executor::execute_with_heartbeat_guarded();
}
