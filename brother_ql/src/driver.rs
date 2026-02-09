use std::{
    fs::File,
    io::{Read, Write},
};

pub struct Printer {
    fd: File,
}

impl Printer {
    pub fn new(path: &str) -> Result<Self, std::io::Error> {
        let fd = File::options().read(true).write(true).open(path)?;

        Ok(Self { fd })
    }

    pub fn read(&mut self, length: usize) -> Result<Vec<u8>, std::io::Error> {
        let mut buf = vec![0u8; length];

        let mut tries = 0;

        while self.fd.read_exact(buf.as_mut_slice()).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tries += 1;

            if tries > 10 {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "Timeout"));
            }
        }

        Ok(buf)
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.fd.write_all(data)?;
        Ok(())
    }
}

#[derive(Debug)]
struct ErrorInformation1 {
    no_media_when_printing: bool,
    end_of_media: bool,
    tape_cutter_jam: bool,
    main_unit_in_use: bool,
    fan_doesnt_work: bool,
}

impl ErrorInformation1 {
    const NO_MEDIA_WHEN_PRINTING: u8 = 0x01;
    const END_OF_MEDIA: u8 = 0x02;
    const TAPE_CUTTER_JAM: u8 = 0x04;
    const MAIN_UNIT_IN_USE: u8 = 0x10;
    const FAN_DOESNT_WORK: u8 = 0x80;

    fn from_bits(bits: u8) -> Self {
        ErrorInformation1 {
            no_media_when_printing: bits & Self::NO_MEDIA_WHEN_PRINTING != 0,
            end_of_media: bits & Self::END_OF_MEDIA != 0,
            tape_cutter_jam: bits & Self::TAPE_CUTTER_JAM != 0,
            main_unit_in_use: bits & Self::MAIN_UNIT_IN_USE != 0,
            fan_doesnt_work: bits & Self::FAN_DOESNT_WORK != 0,
        }
    }
}
#[derive(Debug)]
struct ErrorInformation2 {
    transmission_error: bool,
    cover_opened_while_printing: bool,
    cannot_feed: bool,
    system_error: bool,
}

impl ErrorInformation2 {
    const TRANSMISSION_ERROR: u8 = 0x04;
    const COVER_OPENED_WHILE_PRINTING: u8 = 0x10;
    const CANNOT_FEED: u8 = 0x40;
    const SYSTEM_ERROR: u8 = 0x80;

    fn from_bits(bits: u8) -> Self {
        ErrorInformation2 {
            transmission_error: bits & Self::TRANSMISSION_ERROR != 0,
            cover_opened_while_printing: bits & Self::COVER_OPENED_WHILE_PRINTING != 0,
            cannot_feed: bits & Self::CANNOT_FEED != 0,
            system_error: bits & Self::SYSTEM_ERROR != 0,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum MediaType {
    NoMedia = 0x00,
    Continuous = 0x0A,
    DieCutLabels = 0x0B,
}
#[derive(Debug)]
pub enum StatusType {
    ReplyToStatusRequest,
    PrintingCompleted,
    Error,
    Notification,
    PhaseChange,
}

#[derive(Debug)]
pub enum PhaseState {
    Waiting,
    Printing,
}

#[derive(Debug)]
pub struct PrinterStatus {
    media_width: u8,
    media_length: u8,
    media_type: MediaType,
    error1: ErrorInformation1,
    error2: ErrorInformation2,
    status_type: StatusType,
    phase_state: PhaseState,
}

impl PrinterStatus {
    /// Get the pixel width (print area width in dots) for the loaded media
    pub fn pixel_width(&self) -> Option<u16> {
        match (self.media_width, self.media_length) {
            // Endless tapes (length = 0) - dots_total - offset_r
            (12, 0) => Some(142 - 29),   // 113
            (18, 0) => Some(256 - 171),  // 85
            (29, 0) => Some(342 - 6),    // 336
            (38, 0) => Some(449 - 12),   // 437
            (50, 0) => Some(590 - 12),   // 578
            (54, 0) => Some(636 - 0),    // 636
            (62, 0) => Some(732 - 12),   // 720
            (102, 0) => Some(1200 - 12), // 1188
            (104, 0) => Some(1224 - 12), // 1212

            // Die-cut labels
            (17, 54) => Some(201 - 0),     // 201
            (17, 87) => Some(201 - 0),     // 201
            (23, 23) => Some(272 - 42),    // 230
            (29, 42) => Some(342 - 6),     // 336
            (29, 90) => Some(342 - 6),     // 336
            (38, 90) => Some(449 - 12),    // 437
            (39, 48) => Some(461 - 6),     // 455
            (52, 29) => Some(614 - 0),     // 614
            (54, 29) => Some(630 - 60),    // 570
            (60, 87) => Some(708 - 18),    // 690
            (62, 29) => Some(732 - 12),    // 720
            (62, 100) => Some(732 - 12),   // 720
            (102, 51) => Some(1200 - 12),  // 1188
            (102, 153) => Some(1200 - 12), // 1188
            (104, 164) => Some(1224 - 12), // 1212

            // Round die-cut labels
            // This can't be right
            //(12, 12) => Some(142 - 113), // 29
            (24, 24) => Some(284 - 42),  // 242
            (58, 58) => Some(688 - 51),  // 637

            // Unknown media
            _ => None,
        }
    }
}

#[derive(Clone)]
pub enum PrinterCommandMode {
    /// ESC/P mode (normal)
    EscpNormal = 0x00, // WARNING: THE PDF DOCUMENTATION IS BROKEN AND DOES NOT HAVE THIS VALUES
    /// Raster mode (default)
    Raster = 0x01,
    /// ESC/P mode (text) for QL-650TD
    EscpText = 0x02,
    /// P-touch Template mode for QL-580N/1050/1060N
    PtouchTemplate = 0x03,
}

pub struct PrinterMode {
    /// Auto cut (QL550/560/570/580N/650TD/700/1050/1060N)
    pub auto_cut: bool,
}

pub struct PrinterExpandedMode {
    /// Cut at end (Earlier version of QL-650TD firmware is not supported.)
    pub cut_at_end: bool,
    /// High resolution printing (QL-570/580N/700)
    pub high_resolution_printing: bool,
}

pub enum PrinterCommand {
    /// Reset
    Reset,
    /// Invalid command
    Invalid,
    /// Initialize
    Initialize,
    /// Status info request
    StatusInfoRequest,
    /// Command mode switch (QL-580N/650TD/1050/1060N)
    SetCommandMode(PrinterCommandMode),
    /// Print information command
    SetPrintInformation(PrinterStatus, i32),
    /// Set each mode
    SetMode(PrinterMode),
    /// Specify the page number in ”cut every * labels” (QL-560/570/580N/700/1050/1060N)
    /// When “auto cut” is specified, you can specify page number (1-255) in “cut each *labels”.
    /// Page number = n1 (1- 255)
    /// Default is 1 (cut each label)
    SetPageNumber(u8),
    /// Set expanded mode (QL-560/570/580N/650TD/700/1050/1060N)
    SetExpandedMode(PrinterExpandedMode),
    /// Set margin amount (feed amount)
    SetMarginAmount(u16),
    /// Compression mode selection (QL-570/580N/650TD/1050/1060N
    SetCompressionMode, // todo
    /// Raster graphics transfer
    RasterGraphicsTransfer([u8; 90]), // todo: ql-1050/1060n takes 162 bytes
    /// Zero raster graphics
    ZeroRasterGraphics,
    /// Print command
    Print,
    /// Print command with feeding
    PrintWithFeeding,
    /// Baud rate setting (QL-580N/650TD/1050/1060N)
    SetBaudRate(u16),
}

impl PrinterCommand {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            PrinterCommand::Reset => vec![0x00; 200],
            PrinterCommand::Invalid => vec![0x00],
            PrinterCommand::Initialize => vec![0x1b, 0x40],
            PrinterCommand::StatusInfoRequest => vec![0x1b, 0x69, 0x53],
            PrinterCommand::SetCommandMode(mode) => vec![0x1b, 0x69, 0x61, mode.clone() as u8],
            PrinterCommand::SetPrintInformation(status, line_count) => {
                let flags = 0x02 | 0x04 | 0x08 | 0x40 | 0x80;
                let mut command = vec![
                    0x1b,
                    0x69,
                    0x7a,
                    flags,
                    status.media_type as u8,
                    status.media_width,
                    status.media_length,
                    0,
                    0,
                    0,
                    0,
                    1,
                    0,
                ];
                command[7..11].copy_from_slice(&line_count.to_le_bytes());
                command
            }
            PrinterCommand::SetMode(mode) => vec![0x1b, 0x69, 0x4d, (mode.auto_cut as u8) << 6],
            PrinterCommand::SetPageNumber(page_number) => vec![0x1b, 0x69, 0x41, *page_number],
            PrinterCommand::SetExpandedMode(mode) => vec![
                0x1b,
                0x69,
                0x4B,
                (mode.cut_at_end as u8) << 4 | (mode.high_resolution_printing as u8) << 6,
            ],
            // todo: check endianess
            PrinterCommand::SetMarginAmount(margin) => {
                let mut command = vec![0x1b, 0x69, 0x64, 0, 0];
                command[3..5].copy_from_slice(&margin.to_le_bytes());
                command
            }
            PrinterCommand::SetCompressionMode => vec![0x4d, 0x00],
            PrinterCommand::RasterGraphicsTransfer(data) => {
                let mut command = vec![0x67, 0x00, 90];
                command.extend_from_slice(data);
                command
            }
            PrinterCommand::ZeroRasterGraphics => vec![0x5A],
            PrinterCommand::Print => vec![0x0c],
            PrinterCommand::PrintWithFeeding => vec![0x1A],
            PrinterCommand::SetBaudRate(baud_rate) => {
                vec![0x1b, 0x69, 0x42, *baud_rate as u8, (baud_rate >> 8) as u8]
            }
        }
    }
}

pub struct PrinterCommander {
    printer: Printer,
}

impl PrinterCommander {
    pub fn main(path: &str) -> Result<Self, std::io::Error> {
        let lp = Printer::new(path)?;

        Ok(Self { printer: lp })
    }

    pub fn send_command(&mut self, command: PrinterCommand) -> Result<(), std::io::Error> {
        self.printer.write(&command.to_bytes())
    }

    pub fn read_status(&mut self) -> Result<PrinterStatus, std::io::Error> {
        self.send_command(PrinterCommand::StatusInfoRequest)?;
        let res = self.printer.read(32)?;
        assert!(res[0] == 0x80);
        assert!(res[1] == 0x20);

        let media_type = match res[11] {
            0x00 => MediaType::NoMedia,
            0x0A => MediaType::Continuous,
            0x0B => MediaType::DieCutLabels,
            _ => panic!("Unknown media type"),
        };

        let status_type = match res[18] {
            0x00 => StatusType::ReplyToStatusRequest,
            0x01 => StatusType::PrintingCompleted,
            0x02 => StatusType::Error,
            0x05 => StatusType::Notification,
            0x06 => StatusType::PhaseChange,
            _ => panic!("Unknown status type"),
        };

        let phase_state = match res[19] {
            0x00 => PhaseState::Waiting,
            0x01 => PhaseState::Printing,
            _ => panic!("Unknown phase state"),
        };

        Ok(PrinterStatus {
            media_width: res[10],
            media_type,
            media_length: res[17],
            error1: ErrorInformation1::from_bits(res[8]),
            error2: ErrorInformation2::from_bits(res[9]),
            status_type,
            phase_state,
        })
    }
}
