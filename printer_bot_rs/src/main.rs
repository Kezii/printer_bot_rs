use std::env;

use crate::error::PrinterBotError;
use brother_ql::image::{print_lines, render_image};
use brother_ql::Settings;
use log::*;
use teloxide_core::adaptors::DefaultParseMode;
use teloxide_core::net::Download;
use teloxide_core::types::{ChatId, FileId, Message};
use teloxide_core::Bot;
use teloxide_core::{
    payloads::GetUpdatesSetters,
    requests::{Requester, RequesterExt},
};

mod error;

#[tokio::main]
async fn main() -> Result<(), PrinterBotError> {
    dotenvy::dotenv().ok();
    env_logger::init();

    let token = env::var("BOT_TOKEN").expect("BOT_TOKEN is not set");
    let owner_id: ChatId = ChatId(
        env::var("OWNER_ID")
            .expect("OWNER_ID is not set")
            .parse()
            .expect("invalid OWNER_ID"),
    );

    let bot = teloxide_core::Bot::new(token).parse_mode(teloxide_core::types::ParseMode::Html);

    bot.send_message(owner_id, "sto partendo").await?;

    info!("Started polling");

    let mut offset: u32 = 0;

    let settings = Settings {
        dpi_600: false,
        auto_cut: true,
        dithering: true,
    };

    info!("Settings: {:?}", settings);
    bot.send_message(owner_id, format!("Settings: {:?}", settings))
        .await?;

    loop {
        let updates = bot.get_updates().offset(offset as i32).await;

        match updates {
            Ok(updates) => {
                for update in updates {
                    offset = update.id.0 + 1;

                    if let teloxide_core::types::UpdateKind::Message(message) = update.kind {
                        if message.chat.id != owner_id {
                            //continue;
                        }

                        bot.forward_message(owner_id, message.chat.id, message.id)
                            .await
                            .ok();

                        let res = print_picture(&bot, &message, &settings).await;

                        if res.is_err() {
                            bot.send_message(message.chat.id, format!("Error {:?}", res));
                        }
                    }
                }
            }
            Err(err) => {
                error!("{:?}", err);
                bot.send_message(owner_id, format!("{:#?}", err)).await.ok();
            }
        }
    }
}

async fn print_picture(
    bot: &DefaultParseMode<Bot>,
    message: &Message,
    settings: &Settings,
) -> Result<(), PrinterBotError> {
    if let Some((file_id, file_ext)) = extract_photo_from_message(&bot, &message).await? {
        let file_path = download_file(&bot, &file_id, &file_ext).await?;

        let lines = render_image(&file_path, settings)?;

        if let Err(err) = print_lines(lines, settings) {
            error!("print failed, {:?}", err);
        }
    }

    Ok(())
}

async fn extract_photo_from_message(
    bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    message: &teloxide_core::types::Message,
) -> Result<Option<(String, String)>, PrinterBotError> {
    if let Some(photo) = message.photo() {
        let biggest = photo.iter().max_by_key(|x| x.width);

        if let Some(biggest) = biggest {
            return Ok(Some((biggest.file.id.to_string(), "jpg".to_string())));
        }
    }

    if let Some(sticker) = message.sticker() {
        if sticker.is_static() {
            return Ok(Some((sticker.file.id.to_string(), "webp".to_string())));
        } else {
            bot.send_message(message.chat.id, "Can't print animated stickers")
                .await?;
        }
    }

    if let Some(document) = message.document() {
        if let Some(mime_type) = &document.mime_type {
            let extension = match mime_type.as_ref() {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/tiff" => "tiff",
                "image/bmp" => "bmp",
                _ => {
                    bot.send_message(message.chat.id, "Can't print documents that are not images")
                        .await?;
                    return Ok(None);
                }
            };

            return Ok(Some((document.file.id.to_string(), extension.to_string())));
        }

        return Ok(None);
    }

    Ok(None)
}

async fn download_file(
    bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    file_id: &str,
    file_ext: &str,
) -> Result<String, PrinterBotError> {
    let file = bot.get_file(FileId::from(file_id.to_string())).await?;
    let file_path = format!("/tmp/toprint.{file_ext}");
    let mut dst = tokio::fs::File::create(&file_path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    Ok(file_path)
}
