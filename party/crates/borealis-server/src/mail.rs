//! Email for sign-up: the link that confirms an address, and the notice admins get about a new request.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::{SmtpSettings, SmtpTls};

/// A sent or failed email.
pub type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Sends plain-text email.
pub trait Mailer: Send + Sync {
    /// Send one message.
    fn send<'a>(&'a self, to: &'a str, subject: &'a str, body: String) -> Sending<'a>;
}

/// Email through an SMTP server.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    /// A mailer for the `[smtp]` settings, which [`crate::config::Settings`] has already checked.
    ///
    /// # Errors
    /// When the host or sender can't be used.
    pub fn shared(settings: &SmtpSettings) -> Result<Arc<dyn Mailer>, String> {
        let host = settings.host.as_deref().unwrap_or_default().trim();
        let builder = match settings.tls {
            SmtpTls::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
            SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
            SmtpTls::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                host,
            )),
        }
        .map_err(|e| format!("smtp.host: {e}"))?;
        let mut builder = builder.port(settings.port);
        if let Some(user) = settings.user.as_deref().filter(|u| !u.is_empty()) {
            builder = builder.credentials(Credentials::new(
                user.to_string(),
                settings.password.clone().unwrap_or_default(),
            ));
        }
        let from = settings
            .from
            .as_deref()
            .unwrap_or_default()
            .parse()
            .map_err(|e| format!("smtp.from: {e}"))?;
        Ok(Arc::new(Self {
            transport: builder.build(),
            from,
        }))
    }
}

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, to: &'a str, subject: &'a str, body: String) -> Sending<'a> {
        Box::pin(async move {
            let to: Mailbox = to.parse().map_err(|e| format!("address {to}: {e}"))?;
            let message = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body)
                .map_err(|e| format!("building a message: {e}"))?;
            self.transport
                .send(message)
                .await
                .map(|_| ())
                .map_err(|e| format!("sending: {e}"))
        })
    }
}

/// Keeps what it is asked to send (tests).
#[cfg(test)]
#[derive(Default)]
pub struct Recorder(pub std::sync::Mutex<Vec<(String, String, String)>>);

#[cfg(test)]
impl Mailer for Recorder {
    fn send<'a>(&'a self, to: &'a str, subject: &'a str, body: String) -> Sending<'a> {
        Box::pin(async move {
            self.0.lock().map_err(|e| e.to_string())?.push((
                to.to_string(),
                subject.to_string(),
                body,
            ));
            Ok(())
        })
    }
}

/// The email that confirms an address.
pub fn confirmation(username: &str, link: &str) -> (String, String) {
    (
        "Confirm your Borealis Party email address".to_string(),
        format!(
            "Hello {username},\n\nSomeone, hopefully you, asked for a Borealis Party account with this address. \
             Open this link to confirm it:\n\n{link}\n\nThe link works for 48 hours. If you didn't ask for an \
             account, ignore this email.\n"
        ),
    )
}

/// The email admins get about a new request.
pub fn approval_needed(username: &str, email: &str, admin_page: &str) -> (String, String) {
    (
        format!("Borealis Party: {username} asks for an account"),
        format!(
            "{username} ({email}) asks for an account on your Borealis Party server.\n\n\
             Approve or reject it on the admin page:\n\n{admin_page}\n"
        ),
    )
}
