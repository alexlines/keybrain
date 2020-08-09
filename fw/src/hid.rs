use super::*;

#[derive(Copy, Clone, Debug, FromPrimitive, AsBytes, Unaligned)]
#[repr(u8)]
pub enum HidClassDescriptorType {
    Hid = 0x21,
    Report = 0x22,
    Physical = 0x33,
}

#[derive(Copy, Clone, Debug, FromPrimitive)]
#[repr(u8)]
enum HidRequestCode {
    GetReport = 1,
    GetIdle = 2,
    GetProtocol = 3,
    SetReport = 9,
    SetIdle = 0xA,
    SetProtocol = 0xB,
}

#[derive(Clone, Debug, AsBytes, Unaligned, SmartDefault)]
#[repr(C)]
pub struct HidDescriptor {
    #[default = 9]
    pub length: u8,
    #[default(HidClassDescriptorType::Hid)]
    pub type_: HidClassDescriptorType,
    pub hid_version: U16<LittleEndian>,
    pub country_code: u8,
    #[default = 1]
    pub num_descriptors: u8,
    pub descriptor_type: u8,
    pub descriptor_length: U16<LittleEndian>,
}


#[derive(Debug, Default, Clone)]
pub struct Hid {
}

impl Hid {
    pub fn on_set_config(&mut self, usb: &device::USB) {
        // Prepare empty report for EP1.
        let txoff = get_ep_tx_offset(usb, 1);
        write_usb_sram_16(txoff, 0);
        write_usb_sram_16(txoff + 2, 0);
        write_usb_sram_16(txoff + 4, 0);
        write_usb_sram_16(txoff + 6, 0);
        set_ep_tx_count(usb, 1, 8);

        // Set up EP1 for HID
        usb.epr[1].modify(|r, w| {
            w.ea().bits(1)
                .ep_type().bits(0b11) // INTERRUPT
                .ep_kind().clear_bit() // not used

                // Note: these bits are toggled by writing 1 for some goddamn
                // reason, so we set them as follows. I'd love to extract a
                // utility function for this but svd2rust has ensured that this
                // is impossible.
                .dtog_tx().bit(r.dtog_tx().bit()) // clear bit by toggle
                .stat_tx().bits(r.stat_tx().bits() ^ 0b11) // VALID

                .dtog_rx().bit(r.dtog_rx().bit()) // clear bit by toggle
                .stat_rx().bits(r.stat_rx().bits() ^ 0b01) // STALL (can't receive)
        });
    }

    pub fn on_setup_iface_std(&mut self, setup: &SetupPacket, usb: &device::USB) {
        match (setup.request_type.data_phase_direction(), StdRequestCode::from_u8(setup.request)) {
            (Dir::DeviceToHost, Some(StdRequestCode::GetDescriptor)) => {
                match HidClassDescriptorType::from_u16(setup.value.get() >> 8) {
                    Some(HidClassDescriptorType::Report) => {
                        // HID Report Descriptor
                        let desc = [
                            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01,
                            0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x03, 0x75, 0x01,
                            0x05, 0x08, 0x19, 0x01, 0x29, 0x03, 0x91, 0x02, 0x95, 0x05, 0x75, 0x01, 0x91, 0x01, 0x95, 0x06,
                            0x75, 0x08, 0x26, 0xff, 0x00, 0x05, 0x07, 0x19, 0x00, 0x29, 0x91, 0x81, 0x00, 0xc0,
                        ];
                        write_usb_sram_bytes(get_ep_tx_offset(usb, 0), &desc);
                        // Update transmittable count.
                        set_ep_tx_count(usb, 0, setup.length.get().min(desc.len() as u16));
                    }
                    _ => {
                        // Unknown kind of descriptor.
                        // TODO stall
                        set_ep_tx_count(usb, 0, 0);
                    }
                }
                // Enable transmission.
                configure_response(usb, 0, Status::Valid, Status::Valid);
            }
            _ => {
                // Unsupported
                // Update transmittable count.
                set_ep_tx_count(usb, 0, 0);
                // Set a stall condition.
                configure_response(usb, 0, Status::Stall, Status::Stall);
            }
        }
    }

    pub fn on_setup_iface_class(&mut self, setup: &SetupPacket, usb: &device::USB) {
        match (setup.request_type.data_phase_direction(), HidRequestCode::from_u8(setup.request)) {
            (Dir::HostToDevice, Some(HidRequestCode::SetIdle)) => {
                set_ep_tx_count(usb, 0, 0);
                configure_response(usb, 0, Status::Valid, Status::Valid);
            }
            (Dir::HostToDevice, Some(HidRequestCode::SetReport)) => {
                // whatever
                set_ep_tx_count(usb, 0, 0);
                configure_response(usb, 0, Status::Valid, Status::Valid);
            }
            _ => {
                // Unsupported
                // Update transmittable count.
                set_ep_tx_count(usb, 0, 0);
                // Set a stall condition.
                configure_response(usb, 0, Status::Stall, Status::Stall);
            }
        }
    }

    pub fn on_in(&mut self, ep: usize, usb: &device::USB, keys_down: &[[bool; 2]; 2]) {
        // The host has just read a HID report. Prepare the next one.
        // TODO this introduces one stage of queueing delay; the reports
        // should be generated asynchronously.

        // We have a key status matrix. We want a packed list of keycodes.
        // Scan the matrix to convert.
        let mut write_idx = 2;
        let txoff = get_ep_tx_offset(usb, 1);
        for (dn_row, code_row) in keys_down.iter().zip(&KEYS) {
            for (dn, code) in dn_row.iter().zip(code_row) {
                if *dn {
                    write_usb_sram_8(txoff + write_idx, *code);
                    write_idx += 1;
                }
            }
        }
        // Pad the rest of the report with zeroes.
        while write_idx < 8 {
            write_usb_sram_8(txoff + write_idx, 0);
            write_idx += 1;
        }

        configure_response(usb, ep, Status::Valid, Status::Valid);
    }
}
