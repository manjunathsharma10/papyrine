//! Execute recognized calls against an [`Event`].

use crate::datefmt::{DATE_FORMATS, DateEnv, TIME_FORMATS, date_format_ex, date_keystroke_ex};
use crate::event::{AfError, Event, FieldLookup};
use crate::numfmt::{NumberFormat, number_format, number_keystroke, percent_format};
use crate::recognize::{AfFunc, Arg, Call};
use crate::special::{special_format, special_keystroke, special_keystroke_ex};

fn idx_or_zero(a: &Arg, len: usize) -> usize {
    let i = a.to_int();
    if i >= 0 && (i as usize) < len {
        i as usize
    } else {
        0
    }
}

fn arg(call: &Call, i: usize) -> &Arg {
    const UNDEF: Arg = Arg::Bool(false);
    call.args.get(i).unwrap_or(&UNDEF)
}

/// Run one call. `fields` is consulted only by `AFSimple_Calculate`.
pub fn execute_call(
    call: &Call,
    ev: &mut Event,
    fields: &dyn FieldLookup,
    env: &DateEnv,
) -> Result<(), AfError> {
    match call.func {
        AfFunc::NumberFormat => {
            number_format(
                ev,
                &NumberFormat {
                    n_dec: arg(call, 0).to_int(),
                    sep_style: arg(call, 1).to_int(),
                    neg_style: arg(call, 2).to_int(),
                    currency: arg(call, 4).to_js_string(),
                    prepend: arg(call, 5).truthy(),
                },
            );
        }
        AfFunc::NumberKeystroke => number_keystroke(ev, arg(call, 1).to_int()),
        AfFunc::PercentFormat => {
            percent_format(
                ev,
                arg(call, 0).to_int(),
                arg(call, 1).to_int(),
                arg(call, 2).truthy(),
            )?;
        }
        AfFunc::PercentKeystroke => number_keystroke(ev, arg(call, 1).to_int()),
        AfFunc::DateFormat => {
            let f = DATE_FORMATS[idx_or_zero(arg(call, 0), DATE_FORMATS.len())];
            date_format_ex(ev, f, env);
        }
        AfFunc::DateFormatEx | AfFunc::TimeFormatEx => {
            date_format_ex(ev, &arg(call, 0).to_js_string(), env);
        }
        AfFunc::DateKeystroke => {
            let f = DATE_FORMATS[idx_or_zero(arg(call, 0), DATE_FORMATS.len())];
            date_keystroke_ex(ev, f, env);
        }
        AfFunc::DateKeystrokeEx | AfFunc::TimeKeystrokeEx => {
            date_keystroke_ex(ev, &arg(call, 0).to_js_string(), env);
        }
        AfFunc::TimeFormat => {
            let f = TIME_FORMATS[idx_or_zero(arg(call, 0), TIME_FORMATS.len())];
            date_format_ex(ev, f, env);
        }
        AfFunc::TimeKeystroke => {
            let f = TIME_FORMATS[idx_or_zero(arg(call, 0), TIME_FORMATS.len())];
            date_keystroke_ex(ev, f, env);
        }
        AfFunc::SpecialFormat => special_format(ev, arg(call, 0).to_int()),
        AfFunc::SpecialKeystroke => special_keystroke(ev, arg(call, 0).to_int()),
        AfFunc::SpecialKeystrokeEx => special_keystroke_ex(ev, &arg(call, 0).to_js_string()),
        AfFunc::SimpleCalculate => {
            let names: Vec<String> = match arg(call, 1) {
                Arg::List(v) => v.iter().map(Arg::to_js_string).collect(),
                other => crate::calc::split_field_list(&other.to_js_string()),
            };
            crate::calc::simple_calculate(ev, &arg(call, 0).to_js_string(), &names, fields)?;
        }
        AfFunc::RangeValidate => crate::calc::range_validate(
            ev,
            arg(call, 0).truthy(),
            arg(call, 1).to_number(),
            arg(call, 2).truthy(),
            arg(call, 3).to_number(),
        ),
    }
    Ok(())
}

/// Run a whole recognized script. Stops at the first call that throws.
pub fn execute(
    calls: &[Call],
    ev: &mut Event,
    fields: &dyn FieldLookup,
    env: &DateEnv,
) -> Result<(), AfError> {
    for c in calls {
        execute_call(c, ev, fields, env)?;
    }
    Ok(())
}
