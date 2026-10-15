//! The To, Cc and Bcc fields of a draft. Each recipient is a chip: an atomic token of the text
//! field, so ⌫ takes out the whole recipient and a click selects it (and shows its address).
//! Typing suggests people from the mail ([`jm_core::contacts`]): ↑/↓ pick, ↩, ⇥ or a comma take
//! one, esc closes the list. A typed address becomes a chip at a comma, with ↩ or when leaving
//! the field. The field's text stays `Name <address>, …`.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InlineToken, InlineTokenContext, Input, InputContent, InputEvent, InputState,
    MoveDown, MoveUp,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::Recipient;
use jm_core::contacts::{self, Contact};

use crate::theme;

const SUGGESTIONS: usize = 6;

/// The people each account wrote to or heard from, loaded by the app in the background.
#[derive(Default)]
pub struct AddressBook(HashMap<String, Arc<Vec<Contact>>>);

impl Global for AddressBook {}

impl AddressBook {
    pub fn set(account: String, list: Vec<Contact>, cx: &mut App) {
        cx.default_global::<Self>().0.insert(account, Arc::new(list));
    }

    fn of(account: &str, cx: &App) -> Arc<Vec<Contact>> {
        cx.try_global::<Self>().and_then(|book| book.0.get(account).cloned()).unwrap_or_default()
    }
}

pub struct RecipientField {
    /// Whose address book the suggestions come from.
    account: String,
    input: Entity<InputState>,
    suggestions: Vec<Contact>,
    highlighted: usize,
    _subscription: Subscription,
}

impl RecipientField {
    pub fn new(account: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => this.edited(window, cx),
            InputEvent::Blur => {
                this.suggestions.clear();
                this.make_chips(true, window, cx);
                cx.notify();
            }
            _ => {}
        });
        Self { account, input, suggestions: Vec::new(), highlighted: 0, _subscription: subscription }
    }

    /// The text field, for its change events.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Show `list` as chips (a draft as loaded).
    pub fn set(&mut self, list: &[Recipient], window: &mut Window, cx: &mut Context<Self>) {
        let mut text = String::new();
        let mut chips = Vec::new();
        for recipient in list {
            let token = chip_token(recipient);
            chips.push((text.len()..text.len() + token.text().len(), token.clone()));
            text.push_str(token.text());
        }
        let content = chips
            .into_iter()
            .try_fold(InputContent::new(text.clone()), |content, (range, token)| content.with_token(range, token))
            .unwrap_or_else(|_| InputContent::new(text));
        self.input.update(cx, |s, cx| s.set_value(content, window, cx));
    }

    /// The chips and whatever addresses are typed between them.
    pub fn recipients(&self, cx: &App) -> jm_core::Result<Vec<Recipient>> {
        let state = self.input.read(cx);
        let text = state.value();
        let chips = chip_ranges(state);
        let mut out = Vec::new();
        for (i, gap) in gaps(text.len(), &chips).into_iter().enumerate() {
            out.extend(Recipient::parse_list(&text[gap])?);
            if let Some(chip) = chips.get(i) {
                out.extend(Recipient::parse_list(&text[chip.clone()])?);
            }
        }
        Ok(out)
    }

    /// After every edit: a typed comma takes the highlighted suggestion, an address followed by a
    /// comma becomes a chip, and what is being typed gets suggestions.
    fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, chips, cursor) = self.snapshot(cx);
        if !self.suggestions.is_empty() && text[..cursor].ends_with([',', ';']) {
            let entry = entry_at(&text, &chips, cursor - 1);
            let pick = self.suggestions[self.highlighted].recipient();
            self.suggestions.clear();
            self.apply(vec![(entry.start..cursor, Some(pick))], window, cx);
            return;
        }
        self.make_chips(false, window, cx);
        self.suggest(cx);
    }

    /// Turn typed addresses into chips: those followed by a comma, with `all` the rest too.
    fn make_chips(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (text, chips, _) = self.snapshot(cx);
        self.apply(chip_edits(&text, &chips, all), window, cx);
    }

    fn suggest(&mut self, cx: &mut Context<Self>) {
        let state = self.input.read(cx);
        let (text, chips) = (state.value(), chip_ranges(state));
        let query = text[entry_at(&text, &chips, state.cursor())].trim().to_lowercase();
        let taken: Vec<String> = state.tokens().iter().map(|span| span.token().id().to_string()).collect();
        let book = AddressBook::of(&self.account, cx);
        let mut found: Vec<Contact> = contacts::suggest(&book, &query, &taken, SUGGESTIONS).into_iter().cloned().collect();
        // a fully typed address picks that very contact
        if let Some(exact) = found.iter().position(|c| c.address.eq_ignore_ascii_case(&query)) {
            found.swap(0, exact);
        }
        self.suggestions = found;
        self.highlighted = 0;
        cx.notify();
    }

    /// The highlighted (or clicked) suggestion replaces what is typed.
    fn take(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(contact) = self.suggestions.get(ix).cloned() else { return };
        self.suggestions.clear();
        let (text, chips, cursor) = self.snapshot(cx);
        self.apply(vec![(entry_at(&text, &chips, cursor), Some(contact.recipient()))], window, cx);
        cx.notify();
    }

    fn snapshot(&self, cx: &App) -> (String, Vec<Range<usize>>, usize) {
        let state = self.input.read(cx);
        (state.value().to_string(), chip_ranges(state), state.cursor())
    }

    /// Replace ranges with chips (`None`: drop the range), keeping the cursor where it was.
    fn apply(&mut self, mut edits: Vec<(Range<usize>, Option<Recipient>)>, window: &mut Window, cx: &mut Context<Self>) {
        if edits.is_empty() {
            return;
        }
        // back to front, so the ranges still ahead stay valid
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        self.input.update(cx, |state, cx| {
            let mut cursor = state.cursor();
            for (range, recipient) in edits {
                let len = match recipient {
                    Some(recipient) => {
                        let token = chip_token(&recipient);
                        let len = token.text().len();
                        if state.replace_range_with_token(range.clone(), token, window, cx).is_err() {
                            continue; // an input method is composing; the next edit tries again
                        }
                        len
                    }
                    None => {
                        state.set_selected_range(range.clone(), cx);
                        state.replace("", window, cx);
                        0
                    }
                };
                if cursor >= range.end {
                    cursor = cursor - range.len() + len;
                } else if cursor > range.start {
                    cursor = range.start + len;
                }
            }
            state.set_selected_range(cursor..cursor, cx);
        });
    }

    fn move_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.suggestions.len();
        if n == 0 {
            cx.propagate();
            return;
        }
        self.highlighted = if down { (self.highlighted + 1) % n } else { (self.highlighted + n - 1) % n };
        cx.notify();
    }
}

/// A recipient as a chip: the name, its text `Name <address>, ` (the comma inside, so ⌫ takes it
/// along) and the address as id.
fn chip_token(recipient: &Recipient) -> InlineToken {
    InlineToken::new(recipient.address.to_lowercase(), format!("{}, ", recipient.full())).with_label(recipient.display().to_string())
}

fn chip_ranges(state: &InputState) -> Vec<Range<usize>> {
    state.tokens().iter().map(|span| span.range()).collect()
}

/// The typed text between the chips (the first before them, the last after them).
fn gaps(len: usize, chips: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    for chip in chips {
        out.push(at..chip.start);
        at = chip.end;
    }
    out.push(at..len);
    out
}

/// The entry around `cursor`: its part of the typed text, up to the commas around it.
fn entry_at(text: &str, chips: &[Range<usize>], cursor: usize) -> Range<usize> {
    let Some(gap) = gaps(text.len(), chips).into_iter().find(|g| g.start <= cursor && cursor <= g.end) else {
        return cursor..cursor;
    };
    Recipient::list_parts(&text[gap.clone()])
        .into_iter()
        .map(|part| gap.start + part.start..gap.start + part.end)
        .find(|part| part.start <= cursor && cursor <= part.end)
        .unwrap_or(cursor..cursor)
}

/// Which typed entries turn into chips: each address followed by a comma (the range takes the
/// comma along), with `all` also the last of each gap; an empty entry before a comma goes away.
fn chip_edits(text: &str, chips: &[Range<usize>], all: bool) -> Vec<(Range<usize>, Option<Recipient>)> {
    let mut edits = Vec::new();
    for gap in gaps(text.len(), chips) {
        let parts = Recipient::list_parts(&text[gap.clone()]);
        for (i, part) in parts.iter().enumerate() {
            let next = parts.get(i + 1);
            let entry = text[gap.start + part.start..gap.start + part.end].trim();
            let range = gap.start + part.start..gap.start + next.map_or(part.end, |n| n.start);
            match (next, entry.is_empty()) {
                (None, _) if !all => {}
                (None, true) => {}
                (Some(_), true) => edits.push((range, None)),
                (_, false) => {
                    if let Ok(recipient) = Recipient::parse(entry) {
                        edits.push((range, Some(recipient)));
                    }
                }
            }
        }
    }
    edits
}

fn render_chip(token: &InlineTokenContext, _: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let selected = token.is_selected();
    // selected, it shows the address as well
    let label: SharedString = if selected {
        token.token().text().trim_end_matches([',', ' ']).to_string().into()
    } else {
        token.token().label().clone()
    };
    h_flex().h(token.line_height()).max_w(token.available_width()).pr_1().child(
        div()
            .px_1p5()
            .rounded(px(4.))
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .bg(if selected { theme.primary } else { theme.secondary })
            .text_color(if selected { theme.primary_foreground } else { theme.secondary_foreground })
            .child(label),
    )
    .into_any_element()
}

impl Render for RecipientField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let list = v_flex()
            .absolute()
            .top_full()
            .left_0()
            .mt_1()
            .min_w(px(280.))
            .max_w(px(460.))
            .p_1()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .occlude()
            .children(self.suggestions.iter().enumerate().map(|(ix, contact)| {
                let active = ix == self.highlighted;
                let named = !contact.name.is_empty();
                h_flex()
                    .id(("suggestion", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_baseline()
                    .rounded(px(5.))
                    .text_size(theme::TEXT)
                    .when(active, |d| d.bg(theme.primary).text_color(theme.primary_foreground))
                    .child(if named { contact.name.clone() } else { contact.address.clone() })
                    .when(named, |d| {
                        d.child(
                            div()
                                .text_size(theme::TEXT_SMALL)
                                .text_color(if active { theme.primary_foreground.opacity(0.8) } else { theme.muted_foreground })
                                .child(contact.address.clone()),
                        )
                    })
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.highlighted != ix {
                            this.highlighted = ix;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            // keep the focus in the field (the composer would take it)
                            window.prevent_default();
                            cx.stop_propagation();
                            this.take(ix, window, cx);
                        }),
                    )
            }));
        div()
            .relative()
            .w_full()
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_highlight(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_highlight(false, cx)))
            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                if this.suggestions.is_empty() {
                    this.make_chips(true, window, cx);
                } else {
                    this.take(this.highlighted, window, cx);
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                // ⇥ takes the highlighted suggestion and goes on to the next field as usual
                if this.suggestions.is_empty() {
                    this.make_chips(true, window, cx);
                } else {
                    this.take(this.highlighted, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if !this.suggestions.is_empty() {
                    this.suggestions.clear();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(Input::new(&self.input).appearance(false).px_0().text_size(theme::TEXT).token(render_chip))
            .when(!self.suggestions.is_empty(), |d| d.child(deferred(list).with_priority(1)))
    }
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui_kit brings its own `test` attribute
    use super::{Range, chip_edits, entry_at};

    fn edits(text: &str, chips: &[Range<usize>], all: bool) -> Vec<(Range<usize>, Option<String>)> {
        chip_edits(text, chips, all).into_iter().map(|(r, rec)| (r, rec.map(|r| r.address))).collect()
    }

    #[test]
    fn an_address_becomes_a_chip_at_its_comma() {
        assert_eq!(edits("lena@harbor.example, ", &[], false), [(0..20, Some("lena@harbor.example".into()))]);
        // still typing: nothing yet, unless the field is left
        assert!(edits("lena@harbor.exa", &[], false).is_empty());
        assert_eq!(edits("lena@harbor.exa", &[], true), [(0..15, Some("lena@harbor.exa".into()))]);
        // a name is no address
        assert!(edits("lena, ", &[], false).is_empty());
    }

    #[test]
    fn typed_text_between_chips_is_its_own_entry() {
        // "Tom <tom@b.example>, " is a chip; a pasted list follows it
        let text = "Tom <tom@b.example>, a@b.de; \"Doe, Jane\" <jane@d.example>, ma";
        let chip = 0..21;
        let found = edits(text, std::slice::from_ref(&chip), false);
        assert_eq!(found, [(21..28, Some("a@b.de".into())), (28..58, Some("jane@d.example".into()))]);
        assert_eq!(entry_at(text, std::slice::from_ref(&chip), text.len()), 58..text.len());
        // a stray comma right after a chip goes away
        assert_eq!(edits("Tom <tom@b.example>, ,", &[chip], false), [(21..22, None)]);
    }
}
