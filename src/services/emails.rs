/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Service to send emails

use std::sync::Arc;

use lettre::message::header::ContentType;
use lettre::message::{MessageBuilder, SinglePartBuilder};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParametersBuilder};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::model::config::Configuration;
use crate::utils::apierror::ApiError;
use crate::utils::FaillibleFuture;

/// The service to send emails
pub trait EmailSender {
    /// Sends an email
    fn send_email<'a>(&'a self, to: &'a [String], subject: &'a str, body: String) -> FaillibleFuture<'a, ()>;
}

/// Gets the email sender service
#[must_use]
pub fn get_service(config: Arc<Configuration>) -> Arc<dyn EmailSender + Send + Sync> {
    Arc::new(EmailSenderImpl { config })
}

/// The service to send emails
#[derive(Debug, Clone)]
struct EmailSenderImpl {
    /// The configuration
    config: Arc<Configuration>,
}

impl EmailSender for EmailSenderImpl {
    /// Sends an email
    fn send_email<'a>(&'a self, to: &'a [String], subject: &'a str, body: String) -> FaillibleFuture<'a, ()> {
        Box::pin(async move { self.do_send_email(to, subject, body).await })
    }
}

impl EmailSenderImpl {
    /// Sends an email
    async fn do_send_email(&self, to: &[String], subject: &str, body: String) -> Result<(), ApiError> {
        let mut builder = MessageBuilder::new().message_id(Some(self.generate_msg_id()));
        for to in to {
            builder = builder.to(to.parse()?);
        }
        if let Ok(cc) = self.config.email.cc.parse() {
            builder = builder.cc(cc);
        }
        let email = builder.from(self.config.email.sender.parse()?).subject(subject).singlepart(
            SinglePartBuilder::new()
                .header(ContentType::parse("text/plain; charset=utf-8")?)
                .body(body),
        )?;
        self.send_built_message(email).await
    }

    /// Generates a message id
    fn generate_msg_id(&self) -> String {
        format!("<{}@{}>", uuid::Uuid::new_v4(), self.config.web_domain)
    }

    /// Sends an email over the wire
    async fn send_built_message(&self, email: Message) -> Result<(), ApiError> {
        let tls_parameters = TlsParametersBuilder::new(self.config.email.smtp.host.clone()).build_rustls()?;
        let credentials = Credentials::new(self.config.email.smtp.login.clone(), self.config.email.smtp.password.clone());

        let mailer = (if self.config.email.smtp.port == 25 {
            // old SMTP
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.email.smtp.host)?
        } else if self.config.email.smtp.port == 465 {
            // SMTP with STARTTLS
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.email.smtp.host)?
                .tls(Tls::Opportunistic(tls_parameters))
        } else {
            // 587 and others, always TLS
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.email.smtp.host)?.tls(Tls::Required(tls_parameters))
        })
        .credentials(credentials)
        .build();
        mailer.send(email).await?;
        Ok(())
    }
}
