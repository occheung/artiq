use board_misoc::{csr, ident, clock, uart_logger, i2c, pmp};
use core::ops::Range;


#[derive(Debug)]
pub struct SerdesConfig {
    pub select_odd: u8,
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

fn update_select_odd(eem_pair_no: usize, select_odd: usize) {
    let mut odd_sel_reg = unsafe { csr::eem_transceiver::serdes_select_odd_read() };
    // Clear bit
    odd_sel_reg &= (!(1 << eem_pair_no));
    // Set bit if applicable
    unsafe { csr::eem_transceiver::serdes_select_odd_write(odd_sel_reg | (select_odd << eem_pair_no) as u8) };
}

fn update_invert(eem_pair_no: usize, invert: usize) {
    let mut invert_reg = unsafe { csr::eem_transceiver::serdes_decoder_dly_read() };
    // Clear bit
    invert_reg &= (!(1 << eem_pair_no));
    // Set bit if applicable
    unsafe { csr::eem_transceiver::serdes_decoder_dly_write(invert_reg | (invert << eem_pair_no) as u8) };
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
        // Ensure dly_cnt_in is updated before ld
        clock::spin_us(100);
        assert!(tap == csr::eem_transceiver::serdes_dly_cnt_in_read());
        csr::eem_transceiver::serdes_dly_ld_write(1);
    }
}

pub fn write_config(config: &SerdesConfig) {
    unsafe {
        csr::eem_transceiver::serdes_select_odd_write(config.select_odd as u8);
    }

    for eem_pair_no in 0..4 {
        select_eem_pair(eem_pair_no);
        apply_delay(config.delay[eem_pair_no]);
    }
}

// Find the appropriate delay configuration, in (select_odd, delay_tap, bitslip, flip_order)
fn get_delay(table: &[[bool; 32]]) -> (u8, u8, u8, u8) {
    // Figure out the longest chain of hits within some bitslip & select_odd
    let mut max = 0;
    let mut slip = 0;
    let mut tap = 0;
    for (curr_idx, dly_row) in table.iter().enumerate() {
        let mut curr_len = 0;
        let mut first_hit = 0;
        let mut curr_mid = 0;

        for (dly_tap, dly_stat) in dly_row.iter().enumerate() {
            if *dly_stat {
                // Beginning of a chain of hits
                if curr_len == 0 {
                    first_hit = dly_tap;
                }
                curr_len += 1;
                curr_mid = (dly_tap + first_hit) / 2;
            } else {
                curr_len = 0;
            }

            if curr_len > max {
                max = curr_len;
                slip = curr_idx;
                tap = curr_mid;
            }
        }
    }

    (slip as u8 % 2, tap as u8, (slip as u8 % 10) / 2, slip as u8 / 10)
}

pub unsafe fn assign_delay() -> SerdesConfig {
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

    update_select_odd(0, 0);
    fill_align_table(&mut table);

    for (delay, low_rate) in table.iter().enumerate() {
        println!("{:#02}: {:#010}", delay, low_rate);
    }

    let get_rising_slope = |table: &[f64]| -> Option<Range<usize>> {
        let mut begin = None;
        for (tap, low_rate) in table.iter().enumerate() {
            if *low_rate < 0.1 {
                begin.replace(tap);
            }
            if let Some(begin_tap) = begin {
                if *low_rate > 0.9 {
                    return Some(begin_tap..tap)
                }
            }
        }

        None
    };

    let mut min_deviation = 0.5;
    let mut best_idx = 0;
    let mut start_search_idx = 0;
    loop {
        if let Some(range) = get_rising_slope(&table[start_search_idx..]) {
            println!("Found delay tap range: {:?}", &range);
            if (range.start + start_search_idx) < 5 {
                // The same edge may not appear in other lanes
                start_search_idx += range.end;
                continue;
            }

            for i in range {
                let index = i + start_search_idx;
                let low_rate = table[index];
                let deviance = if low_rate > 0.5 {
                    low_rate - 0.5
                } else {
                    0.5 - low_rate
                };

                if deviance < min_deviation {
                    min_deviation = deviance;
                    best_idx = index;
                }
            }

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
        let mut start_search_idx = 0;
        for dly_delta in -2..=2 {
            let index = (best_idx as i8 + dly_delta) as u8;
            let low_rate = read_align(index);
            let deviance = if low_rate > 0.5 {
                low_rate - 0.5
            } else {
                0.5 - low_rate
            };

            if deviance < min_deviation {
                min_deviation = deviance;
                min_idx = index;
            }
        }

        apply_delay(min_idx);
        delay_list[lane_no] = min_idx;
    }

    SerdesConfig {
        select_odd: 0,
        delay: delay_list,
    }
}

pub unsafe fn assign_bitslip() {
    // Assign bitslip for lane 0
    select_eem_pair(0);

    let mut bitslip = 0;
    for slip in 0..=9 {
        update_invert(0, slip/5);
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

    println!("Apply {} double bitslips", bitslip);

    for lane_no in 1..=3 {
        select_eem_pair(lane_no);

        update_invert(lane_no, bitslip/5);
        for slip in 0..bitslip {
            apply_bitslip();
        }
    }
}
