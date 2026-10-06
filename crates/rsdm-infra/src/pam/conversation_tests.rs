use super::*;
use rsdm_core::ports::AuthError;
use std::collections::VecDeque;

fn data() -> ConversationData {
    ConversationData {
        password: Some(Zeroizing::new(b"secret\0".to_vec())),
        username: CString::new("alice").unwrap(),
        password_answered: false,
        username_answered: false,
        conversation: None,
    }
}

fn ask(data: &mut ConversationData, style: c_int) -> (c_int, *mut PamResponse) {
    let message = PamMessage { msg_style: style, msg: c"prompt".as_ptr() };
    let mut messages = [&message as *const PamMessage];
    let mut responses = std::ptr::null_mut();
    // SAFETY: all pointers reference live objects for the duration of the callback.
    let status = unsafe {
        conversation(1, messages.as_mut_ptr(), &mut responses, (data as *mut ConversationData).cast())
    };
    (status, responses)
}

#[test]
fn password_is_not_reused_for_a_later_challenge() {
    let mut data = data();
    let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
    assert_eq!(status, PAM_SUCCESS);
    // SAFETY: a successful callback returns one allocated response.
    unsafe { free_responses(responses, 1) };

    let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
    assert_eq!(status, PAM_CONV_ERR);
    assert!(responses.is_null());
}

#[test]
fn echoed_username_prompt_receives_the_account_name() {
    let mut data = data();
    let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_ON);
    assert_eq!(status, PAM_SUCCESS);
    // SAFETY: a successful echoed prompt returns one NUL-terminated response.
    unsafe {
        assert_eq!(std::ffi::CStr::from_ptr((*responses).resp), c"alice");
        free_responses(responses, 1);
    }
}

#[derive(Debug)]
struct ScriptedConversation {
    answers: VecDeque<(AuthMessageStyle, Result<Option<PasswordSecret>, AuthError>)>,
}

impl AuthConversation for ScriptedConversation {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        let (style, answer) = self.answers.pop_front().expect("unexpected PAM prompt");
        assert_eq!(message.style, style);
        assert_eq!(message.text, "prompt");
        answer
    }
}

fn assert_answer(data: &mut ConversationData, style: c_int, expected: Option<&str>) {
    let (status, responses) = ask(data, style);
    assert_eq!(status, PAM_SUCCESS);
    // SAFETY: the successful callback returns one response slot, owned by this test.
    unsafe {
        match expected {
            Some(text) => assert_eq!(std::ffi::CStr::from_ptr((*responses).resp).to_str().unwrap(), text),
            None => assert!((*responses).resp.is_null()),
        }
        free_responses(responses, 1);
    }
}

#[test]
fn subsequent_hidden_and_visible_prompts_receive_distinct_responses() {
    let mut data = data();
    data.conversation = Some(Box::new(ScriptedConversation {
        answers: VecDeque::from([
            (AuthMessageStyle::Info, Ok(None)),
            (AuthMessageStyle::Secret, Ok(Some(PasswordSecret::new("123456")))),
            (AuthMessageStyle::Visible, Ok(Some(PasswordSecret::new("challenge answer")))),
            (AuthMessageStyle::Error, Ok(None)),
        ]),
    }));
    assert_answer(&mut data, PAM_PROMPT_ECHO_OFF, Some("secret"));
    assert!(data.password.is_none());
    assert_answer(&mut data, PAM_TEXT_INFO, None);
    assert_answer(&mut data, PAM_PROMPT_ECHO_OFF, Some("123456"));
    assert_answer(&mut data, PAM_PROMPT_ECHO_ON, Some("challenge answer"));
    assert_answer(&mut data, PAM_ERROR_MSG, None);
}

#[test]
fn cancellation_returns_a_conversation_error_without_responses() {
    let mut data = data();
    data.password_answered = true;
    data.conversation = Some(Box::new(ScriptedConversation {
        answers: VecDeque::from([(AuthMessageStyle::Secret, Err(AuthError::InvalidCredentials))]),
    }));
    let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
    assert_eq!(status, PAM_CONV_ERR);
    assert!(responses.is_null());
}

#[test]
fn a_callback_panic_does_not_cross_the_c_boundary() {
    let mut data = data();
    data.password_answered = true;
    data.conversation = Some(Box::new(ScriptedConversation { answers: VecDeque::new() }));
    let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
    assert_eq!(status, PAM_CONV_ERR);
    assert!(responses.is_null());
}

#[test]
fn a_failed_message_batch_does_not_publish_partial_responses() {
    let mut data = data();
    let password = PamMessage { msg_style: PAM_PROMPT_ECHO_OFF, msg: c"prompt".as_ptr() };
    let unsupported = PamMessage { msg_style: 99, msg: c"prompt".as_ptr() };
    let mut messages = [&password as *const PamMessage, &unsupported];
    let mut responses = std::ptr::null_mut();
    // SAFETY: the message array and context remain live throughout the callback.
    let status = unsafe {
        conversation(2, messages.as_mut_ptr(), &mut responses, (&mut data as *mut ConversationData).cast())
    };
    assert_eq!(status, PAM_CONV_ERR);
    assert!(responses.is_null());
    assert!(data.password.is_none());
}
