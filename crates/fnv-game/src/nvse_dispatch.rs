//! The order `nvse::call` tries its handlers in, and a cache of the steps each function
//! name needs (2026-11-21).
//!
//! An extender function is answered by the first of these steps that knows it. A handler
//! whose answer depends on the function's name only (it never declines a name it took
//! before, and declining changes nothing) is *pure*: once it has declined a name it always
//! will, so later calls of that name skip it. The others (*impure*: a decline that can
//! depend on the arguments or the game's state, a hand-off to another dispatch, work before
//! the name is matched) are tried again on every call, in the same order. Measured before
//! it on the Strip with 162 plugins: inside a kNVSE folder condition, walking the handlers
//! was most of `nvse::call`'s time [observed, Time Profiler].
//!
//! The pure flags come from reading each handler (a `match name` whose only decline is the
//! default arm, no `?`, no early return, no state change before the match). **A handler
//! changed so that it declines by its arguments or state must be marked impure here.**
//! `NVF_DISPATCH_CHECK=1` runs every skipped pure step too and panics when one answers a
//! name it declined before: run the tests and a game with it after changing a handler.
#![allow(unused_variables)]

use crate::game::{Game, RunCtx};
use fnv_script::vm::XVal;
use fnv_sim::FormId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

type Answer = Option<Result<XVal, String>>;
type Step = fn(&mut Game, &RunCtx, &str, Option<FormId>, &[XVal]) -> Answer;


fn s0(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::texture_filter::call(g, name, args)
}


fn s1(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_faction_modifiers::call(g, name, args)
}


fn s2(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_actor_queries::call(g, name, on.or(ctx.self_ref), args)
}


fn s3(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    if name == "setcasinowinningslevel" { return Some(crate::casino::set_winnings_level(g, args)); }
    None
}


fn s4(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::delayed::call(g, ctx, name, on, args)
}


fn s5(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    if let Some(v) = crate::script_input::call(&mut g.input, name, args) {
        // EnableKey frees the key whatever holds it, so DisableKeyEx's mask groups on it
        // go too; else a later EnableKeyEx would find a stale group and keep the key
        // disabled [hypothesis: ITR's groups sit on top of DisableKey's one switch].
        if name == "enablekey" {
            if let Some(k) = args.first().and_then(|a| a.num().ok()).and_then(|c| u16::try_from(c as i64).ok()) {
                g.other.itr.key_masks.remove(&k);
            }
        }
        return Some(Ok(v));
    }
    None
}


fn s6(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    // ext-last-core.md: irreversible achievement eligibility, independent of the
    // Hardcore needs switch. The existing saved field feeds IsAlwaysHardcore.
    if name == "disablehardcoretracking" {
        g.player.hardcore_was_off = true;
        return Some(Ok(XVal::Num(0.0)));
    }
    None
}


fn s7(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::ui_projection::call(g, name, subject, args)
}


fn s8(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_lutana::call(g, ctx, name, subject, args)
}


fn s9(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_ttw::call(g, ctx, name, subject, args)
}


fn s10(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_unlisted::call(g, ctx, name, subject, args)
}


fn s11(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_unlisted2::call(g, ctx, name, subject, args)
}


fn s12(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_json::call(g, name, args)
}


fn s13(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_refs::call(g, name, subject, args)
}


fn s14(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_misc::call(g, name, subject, args)
}


fn s15(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_data::call(g, name, subject, args)
}


fn s16(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_notes::call(g, name, subject, args)
}


fn s17(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_nx::call(g, name, subject, args)
}


fn s18(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_actor::call(g, name, subject, args)
}


fn s19(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_items::call(g, ctx, name, subject, args)
}


fn s20(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_amata::call(g, name, subject, args)
}


fn s21(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip::call(g, name, subject, args)
}


fn s22(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_other::call(g, ctx, name, subject, args)
}


fn s23(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg1::call(g, ctx, name, subject, args)
}


fn s24(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg2::call(g, ctx, name, subject, args)
}


fn s25(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg3::call(g, ctx, name, subject, args)
}


fn s26(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg4::call(g, ctx, name, subject, args)
}


fn s27(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg5::call(g, ctx, name, subject, args)
}


fn s28(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg6::call(g, ctx, name, subject, args)
}


fn s29(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg7::call(g, ctx, name, subject, args)
}


fn s30(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg8::call(g, name, subject, args)
}


fn s31(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg9::call(g, name, subject, args)
}


fn s32(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg10::call(g, name, subject, args)
}


fn s33(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup1::call(g, ctx, name, subject, args)
}


fn s34(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_dialogue::call(g, ctx, name, subject, args)
}


fn s35(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup3::call(g, ctx, name, subject, args)
}


fn s36(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup6::call(g, ctx, name, subject, args)
}


fn s37(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup5::call(g, ctx, name, subject, args)
}


fn s38(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup7::call(g, ctx, name, subject, args)
}


fn s39(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jg_nif::call(g, ctx, name, subject, args)
}


fn s40(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sup4::call(g, ctx, name, subject, args)
}


fn s41(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip5::call(g, ctx, name, subject, args)
}


fn s42(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip6::call(g, ctx, name, subject, args)
}


fn s43(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip7::call(g, ctx, name, subject, args)
}


fn s44(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip8::call(g, ctx, name, subject, args)
}


fn s45(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip9::call(g, ctx, name, subject, args)
}


fn s46(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip10::call(g, ctx, name, subject, args)
}


fn s47(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip14::call(g, ctx, name, subject, args)
}


fn s48(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip11::call(g, ctx, name, subject, args)
}


fn s49(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip13::call(g, ctx, name, subject, args)
}


fn s50(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip12::call(g, ctx, name, subject, args)
}


fn s51(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_pp::call(g, name, subject, args)
}


fn s52(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_lu2::call(g, ctx, name, subject, args)
}


fn s53(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_lu3::call(g, ctx, name, subject, args)
}


fn s54(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_showoff5::call(g, ctx, name, subject, args)
}


fn s55(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_showoff2::call(g, ctx, name, subject, args)
}


fn s56(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_showoff3::call(g, ctx, name, subject, args)
}


fn s57(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_showoff4::call(g, ctx, name, subject, args)
}


fn s58(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_anh::call(g, ctx, name, subject, args)
}


fn s59(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_itr1::call(g, ctx, name, subject, args)
}


fn s60(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_itr2::call(g, ctx, name, subject, args)
}


fn s61(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_knvse::call(g, ctx, name, subject, args)
}


fn s62(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jippp::call(g, ctx, name, subject, args)
}


fn s63(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_vheyes::call(g, ctx, name, subject, args)
}


fn s64(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_small::call(g, ctx, name, subject, args)
}


fn s65(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_one111::call(g, name, subject, args)
}


fn s66(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_rest::call(g, ctx, name, subject, args)
}


fn s67(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_console::call(g, name, subject, args)
}


fn s68(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_lists::call(g, name, subject, args)
}


fn s69(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_leveled::call(g, name, args)
}


fn s70(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::mod_files::call(g, name, args)
}


fn s71(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_proj::call(g, name, subject, args)
}


fn s72(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_sound::call(g, name, subject, args)
}


fn s73(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_traits::call(g, name, args)
}


fn s74(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_anim::call(g, name, subject, args)
}


fn s75(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nif_blocks::call(g, name, subject, args)
}


fn s76(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip2::call(g, ctx, name, subject, args)
}


fn s77(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::anims::call_ext(g, name, subject, args)
}


fn s78(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_showoff::call(g, name, subject, args)
}


fn s79(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_hud::call(g, name, subject, args)
}


fn s80(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip3::call(g, ctx, name, subject, args)
}


fn s81(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_jip4::call(g, name, subject, args)
}


fn s82(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_gaps::call(g, ctx, name, subject, args)
}


fn s83(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_plugins::call(g, ctx, name, subject, args)
}


fn s84(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::script_runner::call(g, name, args)
}


// The UI functions, over the menus' tile tree (docs/design/ui-tile-tree.md §5.1);
// before the `set…` setters below, which would read SetUIFloat as a record field's
// setter.
fn s85(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::ui_functions::call(g, name, args)
}


// The rest of the tile functions (task T9; docs/spec-notes/menu-xml.md §9).
fn s86(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::ui_more::call(g, name, subject, args)
}


fn s87(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_settings::call(g, name, args)
}


fn s88(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_tokens::call(g, name, subject, args)
}


fn s89(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_handler_lists::call(g, name, args)
}


fn s90(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let subject = on.or(ctx.self_ref);
    crate::nvse_debug::call(g, name, args)
}


/// Every step in order: the step, whether it is pure, its name for messages.
static STEPS: [(Step, bool, &str); 92] = [
    (s0, false, "texture_filter"),
    (s1, false, "nvse_faction_modifiers"),
    (s2, false, "nvse_actor_queries"),
    (s3, true, "casino"),
    (s4, true, "delayed"),
    (s5, true, "script_input"),
    (s6, true, "hardcore_tracking"),
    (s7, false, "ui_projection"),
    (s8, true, "nvse_lutana"),
    (s9, true, "nvse_ttw"),
    (s10, true, "nvse_unlisted"),
    (s11, false, "nvse_unlisted2"),
    (s12, true, "nvse_json"),
    (s13, true, "nvse_refs"),
    (s14, true, "nvse_misc"),
    (s15, true, "nvse_data"),
    (s16, true, "nvse_notes"),
    (s17, true, "nvse_nx"),
    (s18, true, "nvse_actor"),
    (s19, true, "nvse_items"),
    (s20, true, "nvse_amata"),
    (s21, true, "nvse_jip"),
    (s22, false, "nvse_other"),
    (s23, false, "nvse_jg1"),
    (s24, true, "nvse_jg2"),
    (s25, true, "nvse_jg3"),
    (s26, true, "nvse_jg4"),
    (s27, true, "nvse_jg5"),
    (s28, true, "nvse_jg6"),
    (s29, false, "nvse_jg7"),
    (s30, true, "nvse_jg8"),
    (s31, true, "nvse_jg9"),
    (s32, true, "nvse_jg10"),
    (s33, true, "nvse_sup1"),
    (s34, true, "nvse_dialogue"),
    (s35, true, "nvse_sup3"),
    (s36, true, "nvse_sup6"),
    (s37, true, "nvse_sup5"),
    (s38, true, "nvse_sup7"),
    (s39, true, "nvse_jg_nif"),
    (s40, true, "nvse_sup4"),
    (s41, true, "nvse_jip5"),
    (s42, true, "nvse_jip6"),
    (s43, true, "nvse_jip7"),
    (s44, true, "nvse_jip8"),
    (s45, true, "nvse_jip9"),
    (s46, true, "nvse_jip10"),
    (s47, true, "nvse_jip14"),
    (s48, true, "nvse_jip11"),
    (s49, true, "nvse_jip13"),
    (s50, true, "nvse_jip12"),
    (s51, false, "nvse_pp"),
    (s52, true, "nvse_lu2"),
    (s53, false, "nvse_lu3"),
    (s54, false, "nvse_showoff5"),
    (s55, true, "nvse_showoff2"),
    (s56, false, "nvse_showoff3"),
    (s57, true, "nvse_showoff4"),
    (s58, false, "nvse_anh"),
    (s59, true, "nvse_itr1"),
    (s60, true, "nvse_itr2"),
    (s61, true, "nvse_knvse"),
    (s62, false, "nvse_jippp"),
    (s63, true, "nvse_vheyes"),
    (s64, false, "nvse_small"),
    (s65, false, "nvse_one111"),
    (s66, true, "nvse_rest"),
    (s67, true, "nvse_console"),
    (s68, true, "nvse_lists"),
    (s69, false, "nvse_leveled"),
    (s70, true, "mod_files"),
    (s71, false, "nvse_proj"),
    (s72, true, "nvse_sound"),
    (s73, false, "nvse_traits"),
    (s74, true, "nvse_anim"),
    (s75, true, "nif_blocks"),
    (s76, true, "nvse_jip2"),
    (s77, false, "anims"),
    (s78, true, "nvse_showoff"),
    (s79, true, "nvse_hud"),
    (s80, true, "nvse_jip3"),
    (s81, false, "nvse_jip4"),
    (s82, false, "nvse_gaps"),
    (s83, true, "nvse_plugins"),
    (s84, false, "script_runner"),
    (s85, false, "ui_functions"),
    (s86, false, "ui_more"),
    (s87, true, "nvse_settings"),
    (s88, true, "nvse_tokens"),
    (s89, true, "nvse_handler_lists"),
    (s90, true, "nvse_debug"),
    (crate::nvse::tail, false, "nvse"),
];

/// Per function name, the steps a call tries: the impure steps before the one that answered
/// it first, then that one (all the impure steps when none answered). Process-wide: the
/// route depends on the handlers' code only.
type Routes = HashMap<Box<str>, Arc<[u16]>>;
static ROUTES: LazyLock<Mutex<Routes>> = LazyLock::new(Default::default);

/// `NVF_DISPATCH_CHECK=1` (module docs).
fn check_mode() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NVF_DISPATCH_CHECK").is_ok_and(|v| v.trim() == "1"))
}

/// `nvse::call`: the first step that answers `name`.
pub(crate) fn call(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let known = ROUTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(name).cloned();
    let Some(route) = known else {
        return first_call(g, ctx, name, on, args);
    };
    if check_mode() {
        check(g, ctx, name, on, args, &route);
    }
    for &i in route.iter() {
        if let Some(r) = (STEPS[i as usize].0)(g, ctx, name, on, args) {
            return Some(r);
        }
    }
    // The step that answered before declined this time (an impure one can): the steps
    // after it, as the chain would go on.
    let from = route.last().map_or(0, |&i| i as usize + 1);
    STEPS[from..].iter().find_map(|(step, _, _)| step(g, ctx, name, on, args))
}

/// The whole chain, as before the cache, keeping the route it took.
fn first_call(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal]) -> Answer {
    let mut route: Vec<u16> = Vec::new();
    let mut answer = None;
    for (i, (step, pure, _)) in STEPS.iter().enumerate() {
        if let Some(r) = step(g, ctx, name, on, args) {
            route.push(i as u16);
            answer = Some(r);
            break;
        }
        if !pure {
            route.push(i as u16);
        }
    }
    ROUTES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name.into(), route.into());
    answer
}

/// Check mode: every pure step the route skips still declines `name`.
fn check(g: &mut Game, ctx: &RunCtx, name: &str, on: Option<FormId>, args: &[XVal], route: &[u16]) {
    let last = route.last().map_or(STEPS.len(), |&i| i as usize);
    for (i, (step, pure, label)) in STEPS.iter().enumerate().take(last) {
        if *pure && !route.contains(&(i as u16)) && step(g, ctx, name, on, args).is_some() {
            panic!("nvse_dispatch: step {i} ({label}) answered `{name}`, which it declined before: mark it impure");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::STEPS;

    /// The last step is nvse.rs's own (setters, getters, its match), and it is impure.
    #[test]
    fn the_tail_is_last_and_impure() {
        let (_, pure, label) = STEPS[STEPS.len() - 1];
        assert_eq!(label, "nvse");
        assert!(!pure);
        assert!(STEPS.len() < u16::MAX as usize);
    }
}
