use std::collections::HashMap;
use std::env;
use std::io::Cursor;
use std::time::Instant;

use error::PrinterBotError;
use image::{DynamicImage, ImageBuffer, ImageReader, Luma, Rgba};
use log::*;
use teloxide_core::net::Download;
use teloxide_core::payloads::{SendMessageSetters, SendPhotoSetters};
use teloxide_core::types::{
    ChatId, FileId, InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup,
    InputFile, ReplyMarkup, ReplyParameters,
};
use teloxide_core::{
    payloads::GetUpdatesSetters,
    requests::{Requester, RequesterExt},
};
use tokio::io::split;
use uuid::Uuid;

use crate::driver::{PrinterCommand, PrinterCommandMode, PrinterExpandedMode, PrinterMode};

mod driver;
mod error;

#[derive(Debug)]
struct Settings {
    dpi_600: bool,
    auto_cut: bool,
    dithering: bool,
    public_access: bool,
    owner: ChatId,
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

    let mut image_database: HashMap<u128, FinalImage> = HashMap::new();

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
                            let id = Uuid::new_v4();

                            let raw_image = download_file(&bot, &file_id, &file_ext).await?;

                            let final_image = render_image(raw_image, &settings, id)?;

                            image_database.insert(id.as_u128(), final_image.clone());

                            send_preview(&bot, &message, final_image.clone()).await?;
                        } else {
                            bot.send_message(message.chat.id, "You should send an image")
                                .reply_parameters(ReplyParameters::new(message.id))
                                .await?;
                        }
                    } else if let teloxide_core::types::UpdateKind::CallbackQuery(query) =
                        update.kind
                    {
                        info!("{:?}", query);

                        if let Some(data) = query.data {
                            let c = data.split("_").collect::<Vec<&str>>();

                            if c.len() == 2 && c.get(0) == Some(&"print") {
                                if let Some(s) = c.get(1) {
                                    if let Ok(id) = s.to_owned().parse::<u128>() {
                                        if let Some(image) = image_database.remove(&id) {
                                            if let Err(err) = print_lines(image.lines, &settings) {
                                                error!("print failed, {:?}", err);

                                                if let Some(message) = query.message {
                                                    bot.send_message(
                                                        message.chat().id,
                                                        "Print failed",
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
                                                    "Image invalid, please send again",
                                                )
                                                .reply_parameters(ReplyParameters::new(
                                                    message.id(),
                                                ))
                                                .await?;
                                            }
                                        }
                                    }
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

pub async fn send_preview(
    bot: &teloxide_core::adaptors::DefaultParseMode<teloxide_core::Bot>,
    message: &teloxide_core::types::Message,
    final_image: FinalImage,
) -> Result<(), PrinterBotError> {
    let preview_keyboard = ReplyMarkup::InlineKeyboard(InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("Print", format!("print_{}", final_image.id.as_u128())),
    ]]));

    let mut buffer: Vec<u8> = Vec::new();
    final_image
        .preview
        .write_to(&mut Cursor::new(&mut buffer), image::ImageFormat::Png)?;

    bot.send_photo(message.chat.id, InputFile::memory(buffer))
        .reply_markup(preview_keyboard)
        .await?;

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
) -> Result<DynamicImage, PrinterBotError> {
    let file = bot.get_file(FileId::from(file_id.to_string())).await?;
    let file_path = format!("/tmp/toprint.{file_ext}");
    let mut dst = tokio::fs::File::create(&file_path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    let img = ImageReader::open(file_path)?.decode()?;

    Ok(img)
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
pub struct FinalImage {
    pub id: Uuid,
    pub creation_date: std::time::Instant,
    pub preview: DynamicImage,
    pub lines: Vec<[u8; 90]>, // lines
}

fn render_image(
    img: DynamicImage,
    settings: &Settings,
    id: Uuid,
) -> Result<FinalImage, PrinterBotError> {
    // Limit stickers ratio (so people don't print incredibly long stickers)

    let ratio = img.height() as f32 / img.width() as f32;

    if ratio > 3.5 {
        println!("Ratio is too high: {}", ratio);
        return Err(PrinterBotError::InvalidImage);
    }

    // remove transparency
    let img = img.into_rgba8();

    let background_color = image::Rgba([255, 255, 255, 255]);
    let mut background_image =
        image::ImageBuffer::from_pixel(img.width(), img.height(), background_color);
    image::imageops::overlay(&mut background_image, &img, 0, 0);

    // convert to grayscale

    let img = image::imageops::grayscale(&background_image);

    // resize

    let new_width = 720; //630 per la carta piccola

    let new_height = new_width * img.height() / img.width() * if settings.dpi_600 { 2 } else { 1 };

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

    let lines = img_to_lines(dithered_img)?;

    Ok(FinalImage {
        preview,
        lines,
        id,
        creation_date: Instant::now(),
    })
}

fn print_lines(lines: Vec<[u8; 90]>, settings: &Settings) -> Result<(), PrinterBotError> {
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
        printer.send_command(PrinterCommand::RasterGraphicsTransfer(line))?;
    }

    printer.send_command(PrinterCommand::PrintWithFeeding)?;

    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);
    trace!("{:#?}", printer.read_status()?);

    Ok(())
}
