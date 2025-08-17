use std::{
    fs::File,
    io::{Read, Write},
};

pub struct Printer {
    fd: std::fs::File,
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

#[derive(Debug)]
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
pub struct PrinterCommander {
    printer: Printer,
}

impl PrinterCommander {
    pub fn main(path: &str) -> Result<Self, std::io::Error> {
        let lp = Printer::new(path)?;

        Ok(Self { printer: lp })
    }

    pub fn reset(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x00; 200])
    }

    pub fn initilize(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x1b, 0x40])
    }

    pub fn get_status(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x1b, 0x69, 0x53])
    }

    pub fn set_raster_mode(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x1b, 0x69, 0x61, 0x01])
    }

    pub fn read_status(&mut self) -> Result<PrinterStatus, std::io::Error> {
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

    // pag 20
    pub fn set_print_information(
        &mut self,
        status: PrinterStatus,
        line_count: u32,
    ) -> Result<(), std::io::Error> {
        const FLAGS: u8 = 0x02 | 0x04 | 0x08 | 0x40 | 0x80;

        let mut set_print_info_command = [
            0x1b,
            0x69,
            0x7a,
            FLAGS,
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

        set_print_info_command[7..11].copy_from_slice(&line_count.to_le_bytes());

        self.printer.write(&set_print_info_command)
    }

    pub fn set_margin_amount(&mut self, margin: u16) -> Result<(), std::io::Error> {
        let mut set_margin_amount_command = [0x1b, 0x69, 0x64, 0x00, 0x00];

        set_margin_amount_command[3..5].copy_from_slice(&margin.to_le_bytes());

        self.printer.write(&set_margin_amount_command)
    }

    pub fn raster_line(&mut self, line: &[u8; 90]) -> Result<(), std::io::Error> {
        const LINE_LENGTH: u8 = 90;

        let mut command = vec![0x67, 0x00, LINE_LENGTH];
        command.extend_from_slice(line);

        assert!(line.len() == LINE_LENGTH as usize);

        self.printer.write(&command)
    }
    pub fn print(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x0c])
    }

    pub fn print_last_page(&mut self) -> Result<(), std::io::Error> {
        self.printer.write(&[0x1A])
    }
}
