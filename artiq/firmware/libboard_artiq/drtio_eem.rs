use board_misoc::{csr, clock, config};


struct SerdesConfig {
    pub delay: [u8; 4],
}

impl SerdesConfig {
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                (self as *const SerdesConfig) as *const u8,
                core::mem::size_of::<SerdesConfig>(),
            )
        }
    }
}

fn select_eem_pair(eem_pair_no: usize) {
    unsafe {
        csr::eem_transceiver::serdes_eem_sel_write(eem_pair_no as u8);
    }
}

fn update_invert(invert: bool) {
    unsafe {
        csr::eem_transceiver::serdes_decoder_dly_write(invert as u8);
    }
}

fn apply_bitslip() {
    unsafe {
        csr::eem_transceiver::serdes_bitslip_write(1);
        csr::eem_transceiver::serdes_bitslip_write(1);
    }
}

fn apply_delay(tap: u8) {
    unsafe {
        csr::eem_transceiver::serdes_dly_cnt_in_write(tap);
        clock::spin_us(150);
        csr::eem_transceiver::serdes_dly_ld_write(1);
        clock::spin_us(150);
        assert!(tap == csr::eem_transceiver::serdes_dly_cnt_out_read());
    }
}

fn write_config(config: &SerdesConfig) {
    for eem_pair_no in 0..4 {
        select_eem_pair(eem_pair_no);
        apply_delay(config.delay[eem_pair_no]);
    }
}

fn get_deviation(low_rate: f64) -> f64 {
    if low_rate < 0.5 {
        0.5 - low_rate
    } else {
        low_rate - 0.5
    }
}

unsafe fn assign_delay() -> SerdesConfig {
    let mut table: [f64; 32] = [0.0; 32];

    // Select an appropriate delay for EEM lane 0
    select_eem_pair(0);

    let read_align = |dly: u8| -> f64 {
        apply_delay(dly);
        csr::eem_transceiver::serdes_counter_reset_write(1);
            
        while csr::eem_transceiver::serdes_counter_done_read() == 0 {}

        let (high, low) = (
            csr::eem_transceiver::serdes_counter_high_count_read(),
            csr::eem_transceiver::serdes_counter_low_count_read(),
        );

        (low as f64) / ((high + low) as f64)
    };

    let fill_align_table = |table: &mut [f64]| {
        for delay in 0..32 {
            table[delay as usize] = read_align(delay);
        }
    };

    fill_align_table(&mut table);

    let get_rising_crossover = |table: &[f64]| -> Option<(usize, usize)> {
        let mut begin = None;
        let mut min_deviation = 0.5;
        let mut best_idx = 0;
        for (tap, low_rate) in table.iter().enumerate() {
            if *low_rate < 0.1 {
                begin.replace(tap);
            }

            if begin.is_some() {
                let deviation = get_deviation(*low_rate);
                if deviation < min_deviation {
                    min_deviation = deviation;
                    best_idx = tap;
                }

                // The ratio will not be any closer to 50% after this
                // within the same slope
                if *low_rate >= 0.5 {
                    return Some((best_idx, tap))
                }
            }
        }

        None
    };

    let best_idx;
    let mut start_search_idx = 0;
    loop {
        if let Some((opt_idx, end_tap)) = get_rising_crossover(&table[start_search_idx..]) {
            if opt_idx < 5 {
                // The same edge may not appear in other lanes due to skew
                // 5 taps is very conservative, generally it is 1 or 2
                start_search_idx += end_tap + 1;
                continue;
            }
            best_idx = opt_idx + start_search_idx;
            break;
        } else {
            panic!("No suitable delay tap alignment!")
        }
    }

    apply_delay(best_idx as u8);
    let mut delay_list = [best_idx as u8; 4];

    // Assign delay for other lanes
    for lane_no in 1..=3 {
        select_eem_pair(lane_no);

        let mut min_deviation = 0.5;
        let mut min_idx = 0;
        for dly_delta in -2..=2 {
            let index = (best_idx as i8 + dly_delta) as u8;
            let low_rate = read_align(index);
            let deviation = get_deviation(low_rate);

            if deviation < min_deviation {
                min_deviation = deviation;
                min_idx = index;
            }
        }

        apply_delay(min_idx);
        delay_list[lane_no] = min_idx;
    }

    debug!("DRTIO-over-EEM calibration: {:?}", delay_list);

    SerdesConfig {
        delay: delay_list,
    }
}

unsafe fn assign_bitslip() {
    // Assign bitslip for lane 0
    select_eem_pair(0);

    let mut bitslip = 0;
    for slip in 0..=9 {
        update_invert(slip >= 5);
        clock::spin_us(100);

        csr::eem_transceiver::serdes_reader_reset_write(1);
        clock::spin_us(100);

        if csr::eem_transceiver::serdes_reader_comma_read() == 1 {
            bitslip = slip;
            break;
        } else if slip == 9 {
            panic!("No suitable bitslip found!")
        }

        apply_bitslip();
    }

    debug!("Apply {} double bitslips", bitslip);

    // Copy the lane 0 bitslip to all other lanes
    for lane_no in 1..=3 {
        select_eem_pair(lane_no);

        for _slip in 0..bitslip {
            apply_bitslip();
        }
    }
}

pub fn configure() {
    unsafe {
        config::read("eem_drtio_delay", |r| {
            match r {
                Ok(record) => {
                    info!("Loading DRTIO-over-EEM configuration from flash.");
                    write_config(&*(record.as_ptr() as *const SerdesConfig));
                    assign_bitslip();
                    csr::eem_transceiver::rx_ready_write(1);
                },

                Err(_) => {
                    info!("Calibrate DRTIO-over-EEM...");
                    let config = assign_delay();
            
                    assign_bitslip();
                    csr::eem_transceiver::rx_ready_write(1);

                    config::write("eem_drtio_delay", config.as_bytes()).unwrap();
                }
            }
        })
    }
}
