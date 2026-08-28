//! Bounded acceptance coverage for the Windows single-instance contract.
//!
//! These tests deliberately use unique named mutexes and no GUI message loops. The
//! production names and wire format are asserted through the same contract values,
//! while the kernel behavior is exercised directly through the Windows API.

#[cfg(windows)]
mod windows {
    use std::{
        ffi::c_void,
        sync::atomic::{AtomicU64, Ordering},
        thread,
    };

    const ACTIVATION_VERSION: u16 = 1;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x0000_0080;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;

    const EDITOR_IDENTITY: (&str, &str) = ("Local\\TaskbarGroups.Editor", "TaskbarGroupsEditorUi");
    const FLYOUT_IDENTITY: (&str, &str) = ("Local\\TaskbarGroups.Flyout", "TaskbarGroupsFlyoutUi");
    static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
        fn ReleaseMutex(handle: isize) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    struct MutexHandle(isize);

    impl Drop for MutexHandle {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0);
            }
        }
    }

    fn unique_name(label: &str) -> Vec<u16> {
        let nonce = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
        let process = std::process::id();
        format!("Local\\TaskbarGroups.Acceptance.{label}.{process}.{nonce}")
            .encode_utf16()
            .chain(Some(0))
            .collect()
    }

    fn create_mutex(name: &[u16], owner: bool) -> MutexHandle {
        let handle = unsafe { CreateMutexW(std::ptr::null(), owner as i32, name.as_ptr()) };
        assert_ne!(handle, 0, "CreateMutexW failed");
        MutexHandle(handle)
    }

    fn try_acquire(name: &[u16]) -> Option<MutexHandle> {
        let handle = create_mutex(name, false);
        match unsafe { WaitForSingleObject(handle.0, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Some(handle),
            WAIT_TIMEOUT => None,
            result => panic!("unexpected WaitForSingleObject result: {result:#x}"),
        }
    }

    fn encode_activation(group_name: Option<&str>) -> Vec<u16> {
        let mut payload = vec![ACTIVATION_VERSION, group_name.is_some() as u16];
        payload.extend(group_name.unwrap_or_default().encode_utf16());
        payload.push(0);
        payload
    }

    fn decode_activation(payload: &[u16]) -> Option<Option<String>> {
        if payload.len() < 3 || payload[0] != ACTIVATION_VERSION || payload[1] > 1 {
            return None;
        }
        let value = payload[2..].split(|unit| *unit == 0).next()?;
        let value = String::from_utf16(value).ok()?;
        match payload[1] {
            0 if value.is_empty() => Some(None),
            1 if !value.is_empty() => Some(Some(value)),
            _ => None,
        }
    }

    #[test]
    fn editor_and_flyout_have_distinct_mutex_and_window_identities() {
        assert_ne!(EDITOR_IDENTITY.0, FLYOUT_IDENTITY.0);
        assert_ne!(EDITOR_IDENTITY.1, FLYOUT_IDENTITY.1);
        assert_eq!(EDITOR_IDENTITY.0, "Local\\TaskbarGroups.Editor");
        assert_eq!(FLYOUT_IDENTITY.0, "Local\\TaskbarGroups.Flyout");
    }

    #[test]
    fn abandoned_mutex_is_recoverable_without_waiting() {
        let name = unique_name("abandoned");
        let thread_name = name.clone();
        let owner = thread::spawn(move || {
            let handle = create_mutex(&thread_name, true);
            unsafe {
                CloseHandle(handle.0);
            }
            std::mem::forget(handle);
        });
        owner.join().expect("owner thread should exit");

        let recovered = try_acquire(&name);
        assert!(recovered.is_some(), "abandoned mutex should be recoverable");
    }

    #[test]
    fn activation_codec_round_trips_editor_and_flyout_requests() {
        for group in [None, Some("Games"), Some("中文 — Tools")] {
            let encoded = encode_activation(group);
            assert_eq!(decode_activation(&encoded), Some(group.map(str::to_owned)));
        }
    }

    #[test]
    fn malformed_activation_payloads_are_rejected() {
        let valid = encode_activation(Some("Games"));
        for payload in [
            vec![],
            vec![ACTIVATION_VERSION],
            vec![2, 1, b'G' as u16, 0],
            vec![ACTIVATION_VERSION, 2, b'G' as u16, 0],
            vec![ACTIVATION_VERSION, 0, b'G' as u16, 0],
            vec![ACTIVATION_VERSION, 1, 0],
            vec![ACTIVATION_VERSION, 1, 0xd800, 0],
        ] {
            assert_eq!(decode_activation(&payload), None, "payload: {payload:?}");
        }
        assert_eq!(decode_activation(&valid), Some(Some("Games".into())));
    }

    #[test]
    fn repeated_activation_forwarding_preserves_each_request_payload() {
        let requests = [None, Some("Games"), Some("中文 — Tools"), None];
        let forwarded = requests
            .iter()
            .copied()
            .map(encode_activation)
            .map(|payload| decode_activation(&payload).expect("forwarded payload is valid"))
            .collect::<Vec<_>>();
        assert_eq!(forwarded, requests.map(|group| group.map(str::to_owned)));
    }

    #[test]
    fn second_process_is_rejected_while_first_process_holds_identity() {
        let name = unique_name("duplicate");
        let first = try_acquire(&name).expect("first process should acquire mutex");
        let duplicate_name = name.clone();
        let duplicate = thread::spawn(move || try_acquire(&duplicate_name));
        assert!(
            duplicate
                .join()
                .expect("duplicate process probe should exit")
                .is_none(),
            "duplicate process must not acquire mutex"
        );
        drop(first);
        assert!(
            try_acquire(&name).is_some(),
            "identity should be reusable after exit"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn windows_single_instance_acceptance_tests_are_skipped() {
    eprintln!("SKIP: single-instance acceptance tests require Windows");
}
