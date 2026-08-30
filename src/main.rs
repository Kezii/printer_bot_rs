use std::collections::HashMap;
use std::env;
use std::io::Cursor;
use std::time::Instant;

use base64::prelude::*;
use error::PrinterBotError;
use image::{DynamicImage, ImageBuffer, ImageReader, Luma, Rgba};
use log::*;
use postcard::{from_bytes, to_vec};
use serde::{Deserialize, Serialize};
use teloxide_core::net::Download;
use teloxide_core::payloads::{SendMessageSetters, SendPhotoSetters};
use teloxide_core::types::{
    ChatId, FileId, InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup,
    InputFile, Message, MessageId, ReplyMarkup, ReplyParameters,
};
use teloxide_core::{
    payloads::GetUpdatesSetters,
    requests::{Requester, RequesterExt},
};
use tokio::io::split;
use uuid::Uuid;

use crate::driver::{PrinterCommand, PrinterCommandMode, PrinterExpandedMode, PrinterMode};
use crate::telegram::{download_file, extract_photo_from_message};

mod driver;
mod error;
mod telegram;

#[derive(Debug)]
struct Settings {
    dpi_600: bool,
    auto_cut: bool,
    dithering: bool,
    public_access: bool,
    owner: ChatId,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
enum CallbackCommand {
    Print(u128),
    Rotate(u128),
    Split(u128),
    Delete(u128),
}

impl CallbackCommand {
    pub fn string(&self) -> Result<String, PrinterBotError> {
        let bytes = postcard::to_allocvec(self)?;
        Ok(BASE64_URL_SAFE.encode(bytes))
    }

    pub fn from_string(s: &str) -> Result<CallbackCommand, PrinterBotError> {
        let bytes = BASE64_URL_SAFE.decode(s)?;

        let t = postcard::from_bytes(&bytes)?;

        Ok(t)
    }
}

#[tokio::main]
async fn main() -> Result<(), PrinterBotError> {
    dotenvy::dotenv().ok();
    env_logger::init();

    let token = env::var("BOT_TOKEN").expect("BOT_TOKEN is not set");

    let bot = teloxide_core::Bot::new(token).parse_mode(teloxide_core::types::ParseMode::Html);

    let settings = Settings {
        dpi_600: true,
        auto_cut: true,
        dithering: true,
        public_access: true,
        owner: ChatId(
            env::var("OWNER_ID")
                .expect("OWNER_ID is not set")
                .parse()
                .expect("invalid OWNER_ID"),
        ),
    };

    bot.send_message(settings.owner, "sto partendo").await?;

    info!("Started polling");

    let mut offset: u32 = 0;

    info!("Settings: {:#?}", settings);
    bot.send_message(settings.owner, format!("Settings: {:#?}", settings))
        .await?;

    let mut image_database: HashMap<u128, PipelinedImage> = HashMap::new();

    loop {
        let updates = bot.get_updates().offset(offset as i32).await;

        match updates {
            Ok(updates) => {
                for update in updates {
                    offset = update.id.0 + 1;

                    if let teloxide_core::types::UpdateKind::Message(message) = update.kind {
                        if message.chat.id != settings.owner && !settings.public_access {
                            info!("unauthorized access from {:?}", message.from);
                            continue;
                        }

                        if let Some((file_id, file_ext)) =
                            extract_photo_from_message(&bot, &message).await?
                        {
                            let raw_image = download_file(&bot, &file_id, &file_ext).await?;

                            let mut pipelined = PipelinedImage::new(raw_image, message.chat.id);

                            pipelined.send_preview(&bot, &settings).await?;

                            image_database.insert(pipelined.id.as_u128(), pipelined.clone());
                        } else {
                            bot.send_message(message.chat.id, "You should send an image")
                                .reply_parameters(ReplyParameters::new(message.id))
                                .await?;
                        }
                    } else if let teloxide_core::types::UpdateKind::CallbackQuery(query) =
                        update.kind
                    {
                        info!("{:?}", query);

                        if let Some(data) = &query.data {
                            if let Ok(command) = CallbackCommand::from_string(data) {
                                match command {
                                    CallbackCommand::Print(id) => {
                                        if let Some(image) = image_database.get(&id) {
                                            let render = image.render_image(&settings)?;

                                            if render.1.len() > 2000 {
                                                if let Some(message) = &query.message {
                                                    bot.send_message(
                                                        message.chat().id,
                                                        "the image is too long, please rotate or resize",
                                                    )
                                                    .reply_parameters(ReplyParameters::new(
                                                        message.id(),
                                                    ))
                                                    .await?;
                                                }
                                            } else if let Err(err) =
                                                print_lines(&render.1, &settings)
                                            {
                                                error!("print failed, {:?}", err);

                                                if let Some(message) = query.message {
                                                    bot.send_message(
                                                        message.chat().id,
                                                        format!("print failed, {:?}", err),
                                                    )
                                                    .reply_parameters(ReplyParameters::new(
                                                        message.id(),
                                                    ))
                                                    .await?;
                                                }
                                            }
                                        } else {
                                            if let Some(message) = query.message {
                                                bot.send_message(
                                                    message.chat().id,
                                                    "Image expired, please send again",
                                                )
                                                .reply_parameters(ReplyParameters::new(
                                                    message.id(),
                                                ))
                                                .await?;
                                            }
                                        }
                                    }

                                    CallbackCommand::Rotate(id) => {
                                        if let Some(pip) = image_database.get_mut(&id) {
                                            pip.modifiers.rot += 90;
                                            pip.send_preview(&bot, &settings).await?;
                                        }
                                    }

                                    CallbackCommand::Delete(id) => {
                                        if let Some(mut i) = image_database.remove(&id) {
                                            i.delete_preview(&bot).await;
                                        }
                                    }

                                    CallbackCommand::Split(id) => {}
                                }
                            }
                        }
                    } else {
                        info!("Received non-message update {:?}", update);
                    }
                }
            }
            Err(err) => {
                error!("{:?}", err);
                bot.send_message(settings.owner, format!("{:#?}", err))
                    .await
                    .ok();
            }
        }

        image_database.retain(|_, v| v.creation_date.elapsed().as_secs() < 60 * 30);
    }
}

fn apply_dithering(
    mut input_img: ImageBuffer<Luma<u8>, Vec<u8>>,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, PrinterBotError> {
    // match the brightness of the previous implementation
    let gamma_correction = 5.14;

    input_img
        .pixels_mut()
        .for_each(|x| x.0 = [(255.0 * (x.0[0] as f32 / 255.0).powf(1.0 / gamma_correction)) as u8]);

    use exoquant::*;

    let palette = vec![Color::new(0, 0, 0, 255), Color::new(255, 255, 255, 255)];

    let ditherer = ditherer::FloydSteinberg::vanilla();
    let colorspace = SimpleColorSpace::default();
    let remapper = Remapper::new(&palette, &colorspace, &ditherer);

    let image = input_img
        .pixels()
        .map(|x| Color::new(x.0[0], x.0[0], x.0[0], 255))
        .collect::<Vec<Color>>();

    let indexed_data = remapper.remap(&image, input_img.width() as usize);

    let img = image::ImageBuffer::from_fn(input_img.width(), input_img.height(), |x, y| {
        let i = y * input_img.width() + x;
        let i = indexed_data[i as usize];
        image::Rgba([i * 255, i * 255, i * 255, 255])
    });

    Ok(img)
}

fn apply_threshold(
    mut img: ImageBuffer<Luma<u8>, Vec<u8>>,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, PrinterBotError> {
    img.pixels_mut().for_each(|x| {
        if x.0[0] > 128 {
            x.0[0] = 255;
        } else {
            x.0[0] = 0;
        }
    });

    let img = image::ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
        let i = y * img.width() + x;
        let i = img.get_pixel(x, y).0[0];
        image::Rgba([i, i, i, 255])
    });

    Ok(img)
}

fn img_to_lines(img: ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<[u8; 90]>, PrinterBotError> {
    // convert to vec of line bits
    /*
        let mut lines = Vec::new();

        for y in 0..img.height() {
            let mut line = [0u8; 90];

            for x in 0..img.width() {
                let i = y * img.width() + x;
                let i = indexed_data[i as usize];

                let byte = x / 8;
                let bit = x % 8;

                if i == 0 {
                    line[89 - byte as usize] |= 1 << bit;
                }
            }

            lines.push(line);
        }
    */

    let mut lines = Vec::new();

    for y in 0..img.height() {
        let mut line = [0u8; 90];

        for x in 0..img.width() {
            let i = img.get_pixel(x, y).0[0];

            let byte = x / 8;
            let bit = x % 8;

            // if the pixel is black, set the bit so it's printed (in black)
            if i == 0 {
                line[89 - byte as usize] |= 1 << bit;
            }
        }

        lines.push(line);
    }

    Ok(lines)
}

#[derive(Clone)]
pub struct Modifiers {
    pub rot: i16,
    pub split: bool,
}

#[derive(Clone)]
pub struct PipelinedImage {
    pub id: Uuid,
    pub creation_date: std::time::Instant,
    pub raw_image: DynamicImage,
    pub modifiers: Modifiers,
    pub chat_id: ChatId,
    pub old_preview_id: Option<MessageId>,
}

impl PipelinedImage {
    pub fn new(raw: DynamicImage, chat_id: ChatId) -> Self {
        Self {
            raw_image: raw,
            id: Uuid::new_v4(),
            creation_date: Instant::now(),
            chat_id,
            old_preview_id: None,
            modifiers: Modifiers {
                rot: 0,
                split: false,
            },
        }
    }

    pub fn render_image(
        &self,
        settings: &Settings,
    ) -> Result<(DynamicImage, Vec<[u8; 90]>), PrinterBotError> {
        // Limit stickers ratio (so people don't print incredibly long stickers)

        let image = self.raw_image.clone();

        let image = match self.modifiers.rot % 360 {
            90 => image.rotate90(),
            180 => image.rotate180(),
            270 => image.rotate270(),
            _ => image,
        };

        // remove transparency
        let img = image.clone().into_rgba8();

        let background_color = image::Rgba([255, 255, 255, 255]);
        let mut background_image =
            image::ImageBuffer::from_pixel(img.width(), img.height(), background_color);
        image::imageops::overlay(&mut background_image, &img, 0, 0);

        // convert to grayscale

        let img = image::imageops::grayscale(&background_image);

        // resize

        let new_width = 720; //630 per la carta piccola

        let new_height =
            new_width * img.height() / img.width() * if settings.dpi_600 { 2 } else { 1 };

        let img = image::imageops::resize(
            &img,
            new_width,
            new_height,
            image::imageops::FilterType::Lanczos3,
        );

        let dithered_img = if settings.dithering {
            apply_dithering(img)?
        } else {
            apply_threshold(img)?
        };

        let preview = dithered_img.clone();
        let lines = img_to_lines(dithered_img)?;

        let preview = if settings.dpi_600 {
            let image = DynamicImage::from(preview);
            let image = image.resize_exact(
                image.width() * 2,
                image.height(),
                image::imageops::FilterType::Nearest,
            );
            image
        } else {
            DynamicImage::from(preview)
        };

        Ok((preview, lines))
    }

    pub async fn send_preview(
        &mut self,
        bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
        settings: &Settings,
    ) -> Result<(), PrinterBotError> {
        let render = self.render_image(settings)?;

        let preview = render.0;

        let preview = preview.resize(1000, 1000, image::imageops::FilterType::Lanczos3);

        let preview_keyboard = ReplyMarkup::InlineKeyboard(InlineKeyboardMarkup::new([
            vec![InlineKeyboardButton::callback(
                "Print",
                CallbackCommand::Print(self.id.as_u128()).string()?,
            )],
            vec![
                InlineKeyboardButton::callback(
                    "Rotate",
                    CallbackCommand::Rotate(self.id.as_u128()).string()?,
                ),
                InlineKeyboardButton::callback(
                    "Delete",
                    CallbackCommand::Delete(self.id.as_u128()).string()?,
                ),
            ],
        ]));

        let mut buffer: Vec<u8> = Vec::new();

        preview.write_to(&mut Cursor::new(&mut buffer), image::ImageFormat::Png)?;

        self.delete_preview(bot).await;

        let preview = bot
            .send_photo(self.chat_id, InputFile::memory(buffer))
            .reply_markup(preview_keyboard)
            .await?;

        self.old_preview_id = Some(preview.id);

        Ok(())
    }

    pub async fn delete_preview(
        &mut self,
        bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    ) {
        if let Some(old) = self.old_preview_id.take() {
            bot.delete_message(self.chat_id, old).await.ok();
        }
    }
}

fn print_lines(lines: &Vec<[u8; 90]>, settings: &Settings) -> Result<(), PrinterBotError> {
    let mut printer = driver::PrinterCommander::main("/dev/usb/lp0")?;

    printer.send_command(PrinterCommand::Reset)?;
    printer.send_command(PrinterCommand::Initialize)?;

    // information
    printer.send_command(PrinterCommand::StatusInfoRequest)?;

    let status = printer.read_status()?;
    trace!("{:#?}", status);

    printer.send_command(PrinterCommand::SetCommandMode(PrinterCommandMode::Raster))?;

    printer.send_command(PrinterCommand::SetPrintInformation(
        status,
        lines.len() as i32,
    ))?;

    printer.send_command(PrinterCommand::SetExpandedMode(PrinterExpandedMode {
        cut_at_end: settings.auto_cut,
        high_resolution_printing: settings.dpi_600,
    }))?;

    printer.send_command(PrinterCommand::SetMode(PrinterMode {
        auto_cut: settings.auto_cut,
    }))?;

    // this is needed for the auto cut
    printer.send_command(PrinterCommand::SetPageNumber(1))?;

    printer.send_command(PrinterCommand::SetMarginAmount(0))?;

    debug!("printing {} lines", lines.len());

    for line in lines {
        printer.send_command(PrinterCommand::RasterGraphicsTransfer(line.clone()))?;
    }

    printer.send_command(PrinterCommand::PrintWithFeeding)?;

    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);

    Ok(())
}
