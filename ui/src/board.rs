//! Chessboards share the contract's legal move engine and bundled SVG pieces.

use chess_core::chess::{Board, Color, Move, MoveOutcome, PieceKind, Square};
use dioxus::prelude::*;

fn piece_artwork(set: crate::theme::PieceSet, color: Color, kind: PieceKind) -> &'static str {
    use crate::theme::PieceSet;
    let svg = match (set, color, kind) {
        (PieceSet::Neo, Color::White, PieceKind::Pawn) => {
            include_str!("../assets/pieces/neo-white-pawn.svg")
        }
        (PieceSet::Neo, Color::White, PieceKind::Knight) => {
            include_str!("../assets/pieces/neo-white-knight.svg")
        }
        (PieceSet::Neo, Color::White, PieceKind::Bishop) => {
            include_str!("../assets/pieces/neo-white-bishop.svg")
        }
        (PieceSet::Neo, Color::White, PieceKind::Rook) => {
            include_str!("../assets/pieces/neo-white-rook.svg")
        }
        (PieceSet::Neo, Color::White, PieceKind::Queen) => {
            include_str!("../assets/pieces/neo-white-queen.svg")
        }
        (PieceSet::Neo, Color::White, PieceKind::King) => {
            include_str!("../assets/pieces/neo-white-king.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::Pawn) => {
            include_str!("../assets/pieces/neo-black-pawn.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::Knight) => {
            include_str!("../assets/pieces/neo-black-knight.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::Bishop) => {
            include_str!("../assets/pieces/neo-black-bishop.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::Rook) => {
            include_str!("../assets/pieces/neo-black-rook.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::Queen) => {
            include_str!("../assets/pieces/neo-black-queen.svg")
        }
        (PieceSet::Neo, Color::Black, PieceKind::King) => {
            include_str!("../assets/pieces/neo-black-king.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::Pawn) => {
            include_str!("../assets/pieces/classic-white-pawn.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::Knight) => {
            include_str!("../assets/pieces/classic-white-knight.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::Bishop) => {
            include_str!("../assets/pieces/classic-white-bishop.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::Rook) => {
            include_str!("../assets/pieces/classic-white-rook.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::Queen) => {
            include_str!("../assets/pieces/classic-white-queen.svg")
        }
        (PieceSet::Classic, Color::White, PieceKind::King) => {
            include_str!("../assets/pieces/classic-white-king.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::Pawn) => {
            include_str!("../assets/pieces/classic-black-pawn.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::Knight) => {
            include_str!("../assets/pieces/classic-black-knight.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::Bishop) => {
            include_str!("../assets/pieces/classic-black-bishop.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::Rook) => {
            include_str!("../assets/pieces/classic-black-rook.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::Queen) => {
            include_str!("../assets/pieces/classic-black-queen.svg")
        }
        (PieceSet::Classic, Color::Black, PieceKind::King) => {
            include_str!("../assets/pieces/classic-black-king.svg")
        }
    };
    // Only compile-time SVG artwork is inserted; preferences never become markup.
    svg.split_once('>')
        .unwrap()
        .1
        .trim()
        .strip_suffix("</svg>")
        .unwrap()
}

/// Shared, recolorable artwork for boards, player seats and captures.
#[component]
pub fn PieceImage(kind: PieceKind, color: Color) -> Element {
    use crate::theme::{PieceSet, Theme};
    let theme = use_context::<Signal<Theme>>();
    let set = theme.read().piece_set;
    let artwork = piece_artwork(set, color, kind);
    let style = match color {
        Color::White => "--piece-fill:var(--white-piece,#f9f9f9);--piece-shade:color-mix(in srgb,var(--piece-fill) 90%,#333);--piece-outline:#454545;--piece-detail:#454545;",
        Color::Black => "--piece-fill:var(--black-piece,#555555);--piece-shade:color-mix(in srgb,var(--piece-fill) 80%,#000);--piece-outline:#292929;--piece-detail:#ececec;",
    };
    rsx! {
        svg {
            class: if color == Color::White { "piece piece-svg white" } else { "piece piece-svg black" },
            view_box: if set == PieceSet::Neo { "0 0 50 50" } else { "0 0 45 45" },
            fill: if set == PieceSet::Neo { "var(--piece-outline)" } else { "var(--piece-fill)" },
            fill_rule: "evenodd",
            clip_rule: "evenodd",
            role: "img",
            "aria-label": "{color:?} {kind:?}",
            "data-piece-set": if set == PieceSet::Neo { "neo" } else { "classic" },
            "draggable": "false",
            style,
            dangerous_inner_html: artwork,
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn piece_bitmap_svg(
    theme: &crate::theme::Theme,
    color: Color,
    kind: PieceKind,
    size: u32,
) -> String {
    use crate::theme::PieceSet;
    if !theme.valid() {
        return piece_bitmap_svg(&crate::theme::Theme::default(), color, kind, size);
    }
    let (fill, outline, detail, shade, weight) = match color {
        Color::White => (&theme.white_piece, "#454545", "#454545", 51, 90),
        Color::Black => (&theme.black_piece, "#292929", "#ececec", 0, 80),
    };
    let channels = [1, 3, 5]
        .map(|start| u32::from_str_radix(&fill[start..start + 2], 16).unwrap())
        .map(|channel| (channel * weight + shade * (100 - weight) + 50) / 100);
    let shade = format!("#{:02x}{:02x}{:02x}", channels[0], channels[1], channels[2]);
    let artwork = piece_artwork(theme.piece_set, color, kind)
        .replace("var(--piece-fill)", fill)
        .replace("var(--piece-shade)", &shade)
        .replace("var(--piece-outline)", outline)
        .replace("var(--piece-detail)", detail);
    let view_box = if theme.piece_set == PieceSet::Neo {
        50
    } else {
        45
    };
    let root_fill = if theme.piece_set == PieceSet::Neo {
        outline
    } else {
        fill
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {view_box} {view_box}" fill="{root_fill}" fill-rule="evenodd" clip-rule="evenodd">{artwork}</svg>"#
    )
}

#[derive(Clone, Default, PartialEq)]
struct DragPreviewImages {
    #[cfg(target_arch = "wasm32")]
    images: Vec<(Color, PieceKind, web_sys::HtmlImageElement)>,
}

impl DragPreviewImages {
    fn new(theme: &crate::theme::Theme, size: u32) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let mut images = Vec::new();
            for color in [Color::White, Color::Black] {
                for kind in [
                    PieceKind::Pawn,
                    PieceKind::Knight,
                    PieceKind::Bishop,
                    PieceKind::Rook,
                    PieceKind::Queen,
                    PieceKind::King,
                ] {
                    if let Ok(image) = web_sys::HtmlImageElement::new() {
                        let encoded = js_sys::encode_uri_component(&piece_bitmap_svg(
                            theme, color, kind, size,
                        ))
                        .as_string()
                        .unwrap();
                        image.set_decoding("sync");
                        image.set_src(&format!("data:image/svg+xml,{encoded}"));
                        images.push((color, kind, image));
                    }
                }
            }
            Self { images }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (theme, size);
            Self::default()
        }
    }

    fn set(&self, event: &DragEvent, color: Color, kind: PieceKind, square: Square) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let Some((_, _, image)) = self
                .images
                .iter()
                .find(|(c, k, _)| *c == color && *k == kind)
            else {
                return false;
            };
            if !image.complete() || image.natural_width() == 0 || image.is_connected() {
                return false;
            }
            let data = event.data();
            let Some(native) = data.downcast::<web_sys::DragEvent>() else {
                return false;
            };
            let Some(transfer) = native.data_transfer() else {
                return false;
            };
            let Some(element) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id(&format!("board-square-{square}")))
            else {
                return false;
            };
            let rect = element.get_bounding_client_rect();
            // WebKit uses the image bitmap only while the decoded image is detached.
            transfer.set_drag_image(
                image,
                native.client_x() - rect.left() as i32,
                native.client_y() - rect.top() as i32,
            );
            true
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (event, color, kind, square);
            false
        }
    }
}

fn refresh_drag_preview_size(mut size: Signal<u32>) {
    #[cfg(target_arch = "wasm32")]
    if let Some(square) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("board-square-a8"))
    {
        let pixels = square.get_bounding_client_rect().width().round().max(1.0) as u32;
        if *size.peek() != pixels {
            size.set(pixels);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut size;
}

fn square_order(orientation: Color) -> Vec<Square> {
    let mut squares: Vec<Square> = (0..8)
        .rev()
        .flat_map(|rank| (0..8).map(move |file| Square::from_coords(file, rank).unwrap()))
        .collect();
    if orientation == Color::Black {
        squares.reverse();
    }
    squares
}

fn next_square(square: Square, orientation: Color, key: &str) -> Option<Square> {
    let (file, rank) = match key {
        "ArrowLeft" => (-1, 0),
        "ArrowRight" => (1, 0),
        "ArrowUp" => (0, 1),
        "ArrowDown" => (0, -1),
        _ => return None,
    };
    let direction = if orientation == Color::White { 1 } else { -1 };
    Square::from_coords(
        square.file() as i8 + file * direction,
        square.rank() as i8 + rank * direction,
    )
}

fn focus_element(id: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(id))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = id;
}

#[derive(Clone, Default)]
struct BoardInteraction {
    fen: String,
    color: Option<Color>,
    selected: Option<Square>,
    dragged: Option<Square>,
    promotions: Vec<Move>,
}

#[derive(Clone, Copy)]
enum BoardAction {
    Click(Square),
    DragStart(Square),
    Drop(Square),
    DragEnd,
    Promote(PieceKind),
    Cancel,
}

impl BoardInteraction {
    fn matches(&self, fen: &str, color: Option<Color>) -> bool {
        self.fen == fen && self.color == color
    }

    fn reset_position(&mut self, fen: &str, color: Option<Color>) {
        if !self.matches(fen, color) {
            *self = Self {
                fen: fen.to_string(),
                color,
                ..Self::default()
            };
        }
    }

    fn handle(
        &mut self,
        board: &Board,
        fen: &str,
        color: Option<Color>,
        action: BoardAction,
    ) -> Option<Move> {
        let color = color.filter(|color| *color == board.side_to_move);
        self.reset_position(fen, color);
        if color.is_none() || matches!(action, BoardAction::Cancel) {
            self.selected = None;
            self.dragged = None;
            self.promotions.clear();
            return None;
        }
        if let BoardAction::Promote(kind) = action {
            let mv = self
                .promotions
                .iter()
                .find(|mv| mv.promotion == Some(kind))
                .copied();
            if mv.is_some() {
                self.promotions.clear();
            }
            return mv;
        }
        if !self.promotions.is_empty() {
            return None;
        }
        let movable = |square| {
            board
                .piece_at(square)
                .is_some_and(|piece| Some(piece.color) == color)
        };
        let (from, to) = match action {
            BoardAction::DragStart(square) => {
                self.dragged = movable(square).then_some(square);
                self.selected = self.dragged;
                return None;
            }
            BoardAction::DragEnd => {
                self.dragged = None;
                return None;
            }
            BoardAction::Drop(square) => (self.dragged.take(), square),
            BoardAction::Click(square) => (self.selected, square),
            _ => return None,
        };
        let candidates: Vec<Move> = from
            .map(|from| {
                board
                    .legal_moves_from(from)
                    .into_iter()
                    .filter(|mv| mv.to == to)
                    .collect()
            })
            .unwrap_or_default();
        if !candidates.is_empty() {
            self.selected = None;
            self.dragged = None;
            if candidates.iter().any(|mv| mv.promotion.is_some()) {
                self.promotions = candidates;
                return None;
            }
            return candidates.first().copied();
        }
        self.selected = if movable(to) && self.selected != Some(to) {
            Some(to)
        } else {
            None
        };
        None
    }
}

#[component]
pub fn MiniBoard(fen: String, orientation: Color) -> Element {
    let board = Board::from_fen(&fen).unwrap_or_default();
    rsx! {
        div { class: "board mini", aria_hidden: "true",
            for sq in square_order(orientation) {
                div {
                    key: "{sq.0}",
                    class: if sq.is_light() { "sq light" } else { "sq dark" },
                    if let Some(piece) = board.piece_at(sq) {
                        PieceImage { kind: piece.kind, color: piece.color }
                    }
                }
            }
        }
    }
}

/// `playable: None` keeps replay and spectator boards inert.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn GameBoard(
    fen: String,
    orientation: Color,
    playable: Option<Color>,
    last_move: Option<Move>,
    #[props(default = None)] on_move: Option<EventHandler<Move>>,
) -> Element {
    let theme = use_context::<Signal<crate::theme::Theme>>();
    let preview_size = use_signal(|| 80u32);
    let preview_images = use_memo(move || DragPreviewImages::new(&theme.read(), preview_size()));
    let parsed = Board::from_fen(&fen);
    let color = playable.filter(|color| {
        parsed
            .as_ref()
            .is_ok_and(|board| board.side_to_move == *color)
            && on_move.is_some()
    });
    let board = parsed.unwrap_or_default();
    let mut interaction = use_signal(BoardInteraction::default);
    let mut focused = use_signal(move || square_order(orientation)[0]);
    use_effect(use_reactive((&fen, &color), move |(fen, color)| {
        if !interaction.peek().matches(&fen, color) {
            interaction.write().reset_position(&fen, color);
        }
    }));

    let current = interaction.read();
    let selected = current
        .matches(&fen, color)
        .then_some(current.selected)
        .flatten();
    let promotions = if current.matches(&fen, color) {
        current.promotions.clone()
    } else {
        Vec::new()
    };
    drop(current);
    let promotion_open = !promotions.is_empty();
    let legal_targets = selected
        .map(|from| board.legal_moves_from(from))
        .unwrap_or_default();
    let king_in_check = board
        .is_check()
        .then(|| board.king_square(board.side_to_move))
        .flatten();
    let interact = {
        let board = board.clone();
        let fen = fen.clone();
        EventHandler::new(move |action: BoardAction| {
            let mv = interaction.write().handle(&board, &fen, color, action);
            if let (Some(mv), Some(handler)) = (mv, on_move) {
                handler.call(mv);
            }
        })
    };

    rsx! {
        div { class: "board-container",
            div {
                class: "board",
                onmounted: move |_| refresh_drag_preview_size(preview_size),
                onpointerenter: move |_| refresh_drag_preview_size(preview_size),
                onpointerdown: move |_| refresh_drag_preview_size(preview_size),
                role: "group",
                aria_label: "Chessboard. Use arrow keys to navigate and Enter or Space to select and move. Escape cancels selection.",
                for (index, sq) in square_order(orientation).into_iter().enumerate() {
                    {
                        let target = legal_targets.iter().find(|mv| mv.to == sq);
                        let piece = board.piece_at(sq);
                        let movable = color.is_some() && piece.is_some_and(|piece| Some(piece.color) == color);
                        let capture = target.is_some_and(|mv| piece.is_some() || board.piece_at(mv.from).is_some_and(|piece| piece.kind == PieceKind::Pawn && mv.from.file() != mv.to.file()));
                        let mut class = String::from(if sq.is_light() { "sq light" } else { "sq dark" });
                        if selected == Some(sq) { class.push_str(" sel"); }
                        if last_move.is_some_and(|mv| mv.from == sq || mv.to == sq) { class.push_str(" last"); }
                        if king_in_check == Some(sq) { class.push_str(" check"); }
                        let label = match piece {
                            Some(piece) => format!("{sq}, {:?} {:?}", piece.color, piece.kind),
                            None => format!("{sq}, empty"),
                        };
                        let label = if target.is_some() { format!("{label}, legal destination") } else { label };
                        let piece_code = piece.map(|piece| piece.fen_char().to_string()).unwrap_or_default();
                        let file = (b'a' + sq.file()) as char;
                        let rank = sq.rank() + 1;
                        rsx! {
                            button {
                                key: "{sq.0}",
                                id: "board-square-{sq}",
                                r#type: "button",
                                class,
                                "data-square": "{sq}",
                                "data-piece": piece_code,
                                aria_label: label,
                                aria_pressed: selected == Some(sq),
                                aria_disabled: color.is_none() || promotion_open,
                                tabindex: if focused() == sq { 0 } else { -1 },
                                onfocus: move |_| focused.set(sq),
                                onclick: move |_| interact.call(BoardAction::Click(sq)),
                                onkeydown: move |event| {
                                    let key = event.key().to_string();
                                    if key == "Escape" {
                                        event.prevent_default();
                                        interact.call(BoardAction::Cancel);
                                    } else if key.starts_with("Arrow") {
                                        event.prevent_default();
                                        if let Some(next) = next_square(sq, orientation, &key) {
                                            focused.set(next);
                                            focus_element(&format!("board-square-{next}"));
                                        }
                                    }
                                },
                                ondragstart: move |event| {
                                    if movable && !promotion_open && piece.is_some_and(|piece| preview_images.read().set(&event, piece.color, piece.kind, sq)) {
                                        let _ = event.data_transfer().set_data("text/plain", &sq.to_string());
                                        interact.call(BoardAction::DragStart(sq));
                                    } else {
                                        event.prevent_default();
                                    }
                                },
                                ondragover: move |event| event.prevent_default(),
                                ondrop: move |event| {
                                    event.prevent_default();
                                    interact.call(BoardAction::Drop(sq));
                                },
                                ondragend: move |_| interact.call(BoardAction::DragEnd),
                                if target.is_some() {
                                    span { class: if capture { "hint capture" } else { "hint" }, aria_hidden: "true" }
                                }
                                if let Some(piece) = piece {
                                    // The native drag preview must exclude the square and coordinates.
                                    span {
                                        class: "piece-drag-image",
                                        draggable: movable && !promotion_open,
                                        PieceImage { kind: piece.kind, color: piece.color }
                                    }
                                }
                                if index % 8 == 0 {
                                    span { class: "coord rank", aria_hidden: "true", "{rank}" }
                                }
                                if index >= 56 {
                                    span { class: "coord file", aria_hidden: "true", "{file}" }
                                }
                            }
                        }
                    }
                }
            }
            if promotion_open {
                div { class: "promotion-overlay",
                    div {
                        class: "promotion-card",
                        role: "dialog",
                        aria_modal: "true",
                        aria_label: "Choose a promotion piece",
                        onkeydown: move |event| {
                            if event.key().to_string() == "Escape" {
                                event.prevent_default();
                                interact.call(BoardAction::Cancel);
                                focus_element(&format!("board-square-{}", focused()));
                            }
                        },
                        h3 { "Promote pawn" }
                        div { class: "promotion-options",
                            for (index, mv) in promotions.iter().copied().enumerate() {
                                {
                                    let kind = mv.promotion.unwrap();
                                    let name = format!("{kind:?}");
                                    let count = promotions.len();
                                    let ids: Vec<String> = promotions.iter().map(|mv| format!("promote-{:?}", mv.promotion.unwrap())).chain(std::iter::once("promotion-cancel".to_string())).collect();
                                    rsx! {
                                        button {
                                            key: "{kind:?}",
                                            id: "promote-{kind:?}",
                                            r#type: "button",
                                            title: name.clone(),
                                            aria_label: name,
                                            onmounted: move |_| { if index == 0 { focus_element(&format!("promote-{kind:?}")); } },
                                            onclick: move |_| {
                                                interact.call(BoardAction::Promote(kind));
                                                focus_element(&format!("board-square-{}", focused()));
                                            },
                                            onkeydown: move |event| {
                                                if event.key().to_string() == "Tab" {
                                                    event.prevent_default();
                                                    let next = if event.modifiers().contains(dioxus::html::Modifiers::SHIFT) { (index + count) % (count + 1) } else { (index + 1) % (count + 1) };
                                                    focus_element(&ids[next]);
                                                }
                                            },
                                            PieceImage { kind, color: board.side_to_move }
                                        }
                                    }
                                }
                            }
                        }
                        {
                            let last = promotions.last().unwrap().promotion.unwrap();
                            let first = promotions[0].promotion.unwrap();
                            rsx! {
                                button {
                                    id: "promotion-cancel",
                                    r#type: "button",
                                    onclick: move |_| {
                                        interact.call(BoardAction::Cancel);
                                        focus_element(&format!("board-square-{}", focused()));
                                    },
                                    onkeydown: move |event| {
                                        if event.key().to_string() == "Tab" {
                                            event.prevent_default();
                                            let kind = if event.modifiers().contains(dioxus::html::Modifiers::SHIFT) { last } else { first };
                                            focus_element(&format!("promote-{kind:?}"));
                                        }
                                    },
                                    "Cancel"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Replay outcomes distinguish captures from pawns disappearing on promotion.
pub fn captured_material(outcomes: &[MoveOutcome]) -> (Vec<PieceKind>, Vec<PieceKind>) {
    let render = |color: Color| {
        let mut captured: Vec<PieceKind> = outcomes
            .iter()
            .filter_map(|outcome| outcome.captured)
            .filter(|piece| piece.color == color)
            .map(|piece| piece.kind)
            .collect();
        captured.sort_by_key(|kind| std::cmp::Reverse(kind.material_value()));
        captured
    };
    (render(Color::Black), render(Color::White))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(name: &str) -> Square {
        Square::from_algebraic(name).unwrap()
    }

    #[test]
    fn every_piece_set_bundles_distinct_recolorable_vectors() {
        use crate::theme::PieceSet;
        for set in [PieceSet::Neo, PieceSet::Classic] {
            for color in [Color::White, Color::Black] {
                let mut artwork = std::collections::HashSet::new();
                for kind in [
                    PieceKind::Pawn,
                    PieceKind::Knight,
                    PieceKind::Bishop,
                    PieceKind::Rook,
                    PieceKind::Queen,
                    PieceKind::King,
                ] {
                    let svg = piece_artwork(set, color, kind);
                    assert!(artwork.insert(svg));
                    assert!(svg.contains("var(--piece-fill)"));
                    assert!(!svg.contains("<svg"));
                    assert!(!svg.contains("<image"));
                    assert!(!svg.contains("<script"));
                    assert!(!svg.contains("href="));
                }
            }
        }
    }

    #[test]
    fn drag_bitmaps_are_standalone_and_match_the_piece_palette() {
        use crate::theme::{PieceSet, Theme};
        for set in [PieceSet::Neo, PieceSet::Classic] {
            let theme = Theme {
                piece_set: set,
                white_piece: "#123456".into(),
                black_piece: "#abcdef".into(),
                ..Theme::default()
            };
            for (color, fill, shade) in [
                (Color::White, "#123456", "#153453"),
                (Color::Black, "#abcdef", "#89a4bf"),
            ] {
                let svg = piece_bitmap_svg(&theme, color, PieceKind::Knight, 64);
                assert!(svg.contains("width=\"64\" height=\"64\""));
                assert!(svg.contains(&format!("fill=\"{fill}\"")));
                if set == PieceSet::Neo {
                    assert!(svg.contains(&format!("fill=\"{shade}\"")));
                }
                assert!(!svg.contains("var("));
                assert!(!svg.contains("<rect"));
                assert!(!svg.contains("href="));
            }
        }
        let invalid = Theme {
            white_piece: "red;bad".into(),
            ..Theme::default()
        };
        assert_eq!(
            piece_bitmap_svg(&invalid, Color::White, PieceKind::Pawn, 80),
            piece_bitmap_svg(&Theme::default(), Color::White, PieceKind::Pawn, 80)
        );
    }

    #[test]
    fn board_orientation_and_keyboard_navigation_agree() {
        let white = square_order(Color::White);
        let black = square_order(Color::Black);
        assert_eq!(white.len(), 64);
        assert_eq!(white.first(), Some(&square("a8")));
        assert_eq!(white.last(), Some(&square("h1")));
        assert_eq!(black, white.iter().rev().copied().collect::<Vec<_>>());
        assert_eq!(
            next_square(square("e4"), Color::White, "ArrowUp"),
            Some(square("e5"))
        );
        assert_eq!(
            next_square(square("e4"), Color::Black, "ArrowUp"),
            Some(square("e3"))
        );
        assert_eq!(next_square(square("h4"), Color::White, "ArrowRight"), None);
    }

    #[test]
    fn click_and_drag_submit_only_legal_moves() {
        let board = Board::starting_position();
        let fen = board.to_fen();
        for drag in [false, true] {
            let mut state = BoardInteraction::default();
            let start = if drag {
                BoardAction::DragStart(square("e2"))
            } else {
                BoardAction::Click(square("e2"))
            };
            assert_eq!(state.handle(&board, &fen, Some(Color::White), start), None);
            let finish = if drag {
                BoardAction::Drop(square("e4"))
            } else {
                BoardAction::Click(square("e4"))
            };
            assert_eq!(
                state.handle(&board, &fen, Some(Color::White), finish),
                Move::from_uci("e2e4")
            );
            assert_eq!(state.selected, None);
        }
        let mut state = BoardInteraction::default();
        state.handle(
            &board,
            &fen,
            Some(Color::White),
            BoardAction::Click(square("e2")),
        );
        assert_eq!(
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::Click(square("e5"))
            ),
            None
        );
    }

    #[test]
    fn promotion_waits_for_an_explicit_legal_choice() {
        let board = Board::from_fen("7k/P7/8/8/8/8/8/7K w - - 0 1").unwrap();
        let fen = board.to_fen();
        let mut state = BoardInteraction::default();
        state.handle(
            &board,
            &fen,
            Some(Color::White),
            BoardAction::Click(square("a7")),
        );
        assert_eq!(
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::Click(square("a8"))
            ),
            None
        );
        assert_eq!(state.promotions.len(), 4);
        assert_eq!(
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::Promote(PieceKind::King)
            ),
            None
        );
        assert_eq!(
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::Promote(PieceKind::Knight)
            ),
            Move::from_uci("a7a8n")
        );
        assert!(state.promotions.is_empty());
    }

    #[test]
    fn stale_selection_and_spectator_input_cannot_submit_moves() {
        let board = Board::starting_position();
        let fen = board.to_fen();
        let mut state = BoardInteraction::default();
        state.handle(
            &board,
            &fen,
            Some(Color::White),
            BoardAction::DragStart(square("e2")),
        );
        assert_eq!(
            state.handle(&board, &fen, None, BoardAction::Drop(square("e4"))),
            None
        );
        assert_eq!(state.selected, None);
        state.handle(
            &board,
            &fen,
            Some(Color::White),
            BoardAction::Click(square("e2")),
        );
        let changed = format!("{fen} ");
        assert_eq!(
            state.handle(
                &board,
                &changed,
                Some(Color::White),
                BoardAction::Click(square("e4"))
            ),
            None
        );
        assert_eq!(
            state.handle(
                &board,
                &fen,
                Some(Color::Black),
                BoardAction::Click(square("e7"))
            ),
            None
        );
        assert_eq!(state.selected, None);
    }

    #[test]
    fn cancelled_or_stale_promotion_cannot_emit_a_move() {
        let board = Board::from_fen("7k/P7/8/8/8/8/8/7K w - - 0 1").unwrap();
        let fen = board.to_fen();
        let mut state = BoardInteraction::default();
        for cancelled in [true, false] {
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::DragStart(square("a7")),
            );
            state.handle(
                &board,
                &fen,
                Some(Color::White),
                BoardAction::Drop(square("a8")),
            );
            assert_eq!(state.promotions.len(), 4);
            if cancelled {
                state.handle(&board, &fen, Some(Color::White), BoardAction::Cancel);
            } else {
                state.reset_position(&fen, None);
            }
            assert_eq!(
                state.handle(
                    &board,
                    &fen,
                    Some(Color::White),
                    BoardAction::Promote(PieceKind::Queen)
                ),
                None
            );
            assert!(state.promotions.is_empty());
        }
    }

    #[test]
    fn captured_material_tracks_captures_and_ignores_promotion() {
        let mut game = chess_core::chess::Game::new();
        assert_eq!(captured_material(game.outcomes()), (vec![], vec![]));
        for mv in ["e2e4", "d7d5", "e4d5"] {
            game.play(Move::from_uci(mv).unwrap()).unwrap();
        }
        assert_eq!(
            captured_material(game.outcomes()),
            (vec![PieceKind::Pawn], vec![])
        );
        assert_eq!(captured_material(&game.outcomes()[..2]), (vec![], vec![]));
        let board = Board::from_fen("7k/P7/8/8/8/8/8/7K w - - 0 1").unwrap();
        let mut promotion = chess_core::chess::Game::from_board(board);
        promotion.play(Move::from_uci("a7a8q").unwrap()).unwrap();
        assert_eq!(captured_material(promotion.outcomes()), (vec![], vec![]));
    }
}
