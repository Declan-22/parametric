use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, rgba};
use crate::core::constraints::{ConstraintKind, DimTarget, ElementRef};
use crate::core::document::{Segment, StrokeCap, StrokeDash};
use crate::theme::{Theme, lerp_rgb};

mod color_picker;

pub const INSPECTOR_WIDTH: f32 = 260.0;

#[derive(IntoElement)]
pub struct Inspector {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Inspector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let Some(entity) = self.editor.upgrade() else { return div(); };
        let (selected, bounds, segment, fill, constraints, modifiers, color_picker_open, show_grid, snap_to_grid, snap_to_objects) = {
            let ed = entity.read(cx);
            let selected = ed.selection.clone();
            let points = ed.doc.selection_points(&selected);
            let bounds = ed.doc.bounds_of_points(points.iter());
        let segment = selected.iter().find_map(|e| e.as_segment().and_then(|id| ed.doc.segment(id)));
        let fill = selected.iter().find_map(|e| e.as_fill().and_then(|id| ed.doc.fill(id).cloned()));
            let constraints: Vec<_> = ed.doc.constraints.iter().copied().filter(|c|
                points.contains(&c.a) || points.contains(&c.b) ||
                c.tangent_segments.is_some_and(|(a,b)| selected.contains(&ElementRef::Segment(a)) || selected.contains(&ElementRef::Segment(b)))
            ).collect();
            let selected_segments: Vec<_> = selected.iter().filter_map(|e| e.as_segment()).collect();
            let dimensions: Vec<_> = ed.doc.dimensions.iter().enumerate().filter(|(_, d)| match d.target {
                DimTarget::Points { a, b, .. } => points.contains(&a) || points.contains(&b),
                DimTarget::EdgeMid { a, b, .. } | DimTarget::Lines { a, b } | DimTarget::Angle { a, b } => selected_segments.contains(&a) || selected_segments.contains(&b),
                DimTarget::PointLine { p, line } => points.contains(&p) || selected_segments.contains(&line),
                DimTarget::Radius { seg } | DimTarget::CurveLength { seg } => selected_segments.contains(&seg),
            }).map(|(index, d)| (index, format!("Dimension · {:.2}", d.value))).collect::<Vec<_>>();
            let modifiers = ed.doc.modifiers.iter().enumerate().filter(|(_, m)| selected_segments.contains(&m.first) || selected_segments.contains(&m.second)).map(|(i,m)|(i,m.radius,m.side)).collect::<Vec<_>>();
            (selected, bounds, segment, fill, (constraints, dimensions), modifiers, ed.color_picker_open, ed.show_grid, ed.snap_to_grid, ed.snap_to_objects)
        };
        if let Some(shell) = self.shell.upgrade() {
            let _ = shell.update(cx, |shell, _| {
                for (key, on) in [("inspector-show-grid", show_grid), ("inspector-snap-to-grid", snap_to_grid), ("inspector-snap-to-objects", snap_to_objects)] {
                    shell.fades.entry(key.to_string()).or_insert(if on { 1.0 } else { 0.0 });
                }
            });
        }
        let mut root = div().absolute().right_0().top_0().bottom_0().w(px(INSPECTOR_WIDTH))
            .flex().flex_col().gap(px(8.)).py(px(8.)).bg(rgb(t.bg_primary))
            .border_l_1().border_color(rgb(t.component_border_color))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if selected.is_empty() {
            root = root
                .child(Self::section("Grid", t, Self::grid(&self.editor, t, show_grid)))
                .child(Self::section("Snapping", t, Self::snapping(&self.editor, t, snap_to_grid, snap_to_objects)));
        } else {
            root = root.child(Self::section("Geometry", t, Self::geometry(&self.editor, t, bounds)))
                .child(Self::section("Constraints", t, self.constraints(constraints, t)))
                .child(Self::section("Modifiers", t, self.modifiers(modifiers, t)))
                .child(Self::section("Appearance", t, Self::appearance(&self.editor, t, segment, fill, color_picker_open, cx)));
        }
        root
    }
}

impl Inspector {
    fn section<T: IntoElement>(title: &'static str, t: Theme, body: T) -> impl IntoElement {
        div().flex().flex_col().gap(px(8.)).child(div().px(px(8.)).text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(t.text_secondary)).child(title)).child(body)
            .child(div().h(px(1.)).w_full().bg(rgb(t.component_border_color)))
    }

    fn geometry(editor: &gpui::WeakEntity<crate::editor::Editor>, t: Theme, bounds: Option<crate::core::geometry::Rect>) -> impl IntoElement {
        let (x,y,w,h) = bounds.map(|b|(b.origin.x,b.origin.y,b.size.w,b.size.h)).unwrap_or((0.,0.,0.,0.));
        let e=editor.clone(); let f=editor.clone();
        div().flex().flex_col().gap(px(5.))
            .child(Self::field("X",format!("{x:.2}"),editor.clone(),crate::editor::InspectorField::X,t)).child(Self::field("Y",format!("{y:.2}"),editor.clone(),crate::editor::InspectorField::Y,t))
            .child(Self::field("Width",format!("{w:.2}"),editor.clone(),crate::editor::InspectorField::Width,t)).child(Self::field("Height",format!("{h:.2}"),editor.clone(),crate::editor::InspectorField::Height,t))
            .child(Self::clickable("↔  Flip horizontal",e,|ed,cx|ed.inspector_flip(true,cx),t))
            .child(Self::clickable("↕  Flip vertical",f,|ed,cx|ed.inspector_flip(false,cx),t))
    }

    fn field(label: &'static str, value: String, editor: gpui::WeakEntity<crate::editor::Editor>, field: crate::editor::InspectorField, t: Theme) -> impl IntoElement {
        let input = value.clone();
        div().flex().items_center().justify_between().px(px(10.)).child(div().text_sm().text_color(rgb(t.text_secondary)).child(label)).child(div().id(label).w(px(92.)).h(px(28.)).px(px(7.)).flex().items_center().bg(rgb(t.bg_tertiary)).border_1().border_color(rgb(t.border_color)).rounded(px(8.)).shadow(vec![t.shadow_sm()]).text_xs().cursor_text().on_mouse_down(MouseButton::Left,move|_,_,cx|{let _=editor.update(cx,|e,c|e.begin_inspector_input(field,input.clone(),c));}).child(value))
    }

    fn appearance(editor: &gpui::WeakEntity<crate::editor::Editor>, t: Theme, segment: Option<Segment>, fill: Option<crate::core::document::Fill>, color_picker_open: bool, cx: &App) -> impl IntoElement {
        let (color,width,opacity,dash,cap)=segment.map(|s|(s.stroke_color,s.stroke_width,s.opacity,s.dash,s.cap)).unwrap_or((0x202124,1.,1.,StrokeDash::Solid,StrokeCap::Butt));
        let picker_color = segment.map(|s| s.stroke_color).or_else(|| fill.as_ref().map(|f| f.fill_color)).unwrap_or(color);
        let a=editor.clone(); let b=editor.clone(); let c=editor.clone(); let d=editor.clone();
        let fill_row = fill.as_ref().map(|f| {
            let editor = editor.clone();
            Self::appearance_row("Fill", format!("#{:06X}", f.fill_color), editor, |_e,_c| {}, t, Some(f.fill_color), Some(crate::editor::InspectorField::FillHex))
        });
        let remove_fill = if fill.is_some() { Some(Self::clickable("Remove fill", editor.clone(), |e,c|e.inspector_remove_selected_fills(c), t)) } else { None };
        let mut body = div().flex().flex_col().gap(px(5.));
        if let Some(row) = fill_row { body = body.child(row); }
        if let Some(remove) = remove_fill { body = body.child(remove); }
        body
            .child(Self::section_rule(t))
            .child(Self::appearance_row("Stroke",format!("#{color:06X}"),a,|_,_| {},t,Some(color),Some(crate::editor::InspectorField::StrokeHex)))
            .child(Self::appearance_row("Width",format!("{width:.2} px"),b,|e,c|e.inspector_stroke_width(0.5,c),t,None,None))
            .child(Self::appearance_row("Opacity",format!("{}%",(opacity*100.) as i32),c,|e,c|e.inspector_cycle_opacity(c),t,None,Some(crate::editor::InspectorField::Opacity)))
            .child(Self::appearance_row("Pattern",if dash==StrokeDash::Solid {"Solid".into()} else {"Dashed".into()},d,|e,c|e.inspector_cycle_stroke(c),t,None,None))
            .children(if color_picker_open { Some(color_picker::render(editor.clone(), picker_color, t, cx)) } else { None })
            .child(div().flex().items_center().justify_between().px(px(10.)).child(div().text_xs().text_color(rgb(t.text_secondary)).child("Cap")).child(div().text_xs().text_color(rgb(t.text_primary)).child(match cap {StrokeCap::Butt=>"Butt",StrokeCap::Round=>"Round",StrokeCap::Square=>"Square"})))
    }

    fn section_rule(t: Theme) -> impl IntoElement { div().h(px(1.)).w_full().bg(rgb(t.component_border_color)) }


    fn appearance_row(label: &'static str, value: String, editor: gpui::WeakEntity<crate::editor::Editor>, f: impl Fn(&mut crate::editor::Editor,&mut gpui::Context<crate::editor::Editor>)+'static, t: Theme, swatch: Option<u32>, input_field: Option<crate::editor::InspectorField>) -> impl IntoElement {
        let input = value.clone();
        div().flex().items_center().justify_between().gap(px(8.)).h(px(28.)).pl(px(6.)).pr(px(4.)).mx(px(8.)).rounded(px(8.)).cursor_pointer().bg(rgb(t.bg_tertiary)).border_1().border_color(rgb(t.border_color)).shadow(vec![t.shadow_sm()])
            .on_mouse_down(MouseButton::Left,move|_,_,cx|{let _=editor.update(cx,|e,c|{ if matches!(input_field, Some(crate::editor::InspectorField::StrokeHex) | Some(crate::editor::InspectorField::FillHex)) { e.toggle_color_picker(c); } else { f(e,c); } if let Some(field)=input_field { e.begin_inspector_input(field,input.clone(),c); }}); })
            .child(div().text_xs().text_color(rgb(t.text_secondary)).child(label))
            .child(div().flex().items_center().gap(px(6.)).child(swatch.map(|c|div().w(px(18.)).h(px(18.)).rounded(px(4.)).bg(rgba((c<<8)|0xff)).border_1().border_color(rgb(t.border_color))).unwrap_or_else(||div())).child(div().text_xs().text_color(rgb(t.text_primary)).child(value)))
    }

    fn constraints(&self, (list, dimensions): (Vec<crate::core::constraints::Constraint>, Vec<(usize, String)>), t: Theme) -> impl IntoElement {
        let mut body=div().flex().flex_col().gap(px(3.)).px(px(10.));
        if list.is_empty() && dimensions.is_empty() { body=body.child(div().text_xs().text_color(rgb(t.text_secondary)).child("No constraints attached")); }
        for c in list { let editor=self.editor.clone(); let x=c; body=body.child(Self::constraint_row(match x.kind {ConstraintKind::Coincident=>"Coincident",ConstraintKind::Horizontal=>"Horizontal",ConstraintKind::Vertical=>"Vertical",ConstraintKind::Tangent=>"Tangent",ConstraintKind::Parallel=>"Parallel",ConstraintKind::Perpendicular=>"Perpendicular"},format!("P{} · P{}",x.a.idx,x.b.idx),editor,move|ed,cx|ed.inspector_remove_constraint(x,cx),t)); }
        for (index, dimension) in dimensions { let editor=self.editor.clone(); body=body.child(Self::constraint_row("Dimension",dimension,editor,move|ed,cx|ed.inspector_remove_dimension(index,cx),t)); }
        body
    }

    fn modifiers(&self, list: Vec<(usize, f64, crate::core::fillet::FilletSide)>, t: Theme) -> impl IntoElement {
        let mut body = div().flex().flex_col().gap(px(3.)).px(px(10.));
        if list.is_empty() { return body.child(div().text_xs().text_color(rgb(t.text_secondary)).child("No modifiers attached")); }
        for (index, radius, side) in list {
            let editor = self.editor.clone();
            let editor_side = self.editor.clone();
            let editor_del = self.editor.clone();
            // The R value reopens the radius input (works on loaded
            // fillets via the persisted arc id); the side chip flips the
            // Inner/Outer wedge; × deletes.
            body = body.child(
                div().flex().items_center().justify_between().gap(px(8.)).h(px(28.)).px(px(7.))
                    .rounded(px(8.)).bg(rgb(t.bg_tertiary)).border_1()
                    .border_color(rgb(t.border_color)).shadow(vec![t.shadow_sm()])
                    .child(
                        div().flex().items_center().gap(px(6.))
                            .child(div().text_xs().text_color(rgb(t.text_primary)).child("Fillet"))
                            .child(
                                div().text_xs().text_color(rgb(t.text_secondary)).cursor_pointer()
                                    .child(format!("R {:.2}", radius))
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                        let _ = editor.update(cx, |ed, _| { ed.open_fillet_radius_input(index); });
                                    }),
                            )
                            .child(
                                div().text_xs().px(px(6.)).py(px(2.)).rounded(px(5.))
                                    .bg(rgb(t.bg_primary)).border_1().border_color(rgb(t.component_border_color))
                                    .text_color(rgb(t.text_secondary)).cursor_pointer()
                                    .child(side.as_str())
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                        let _ = editor_side.update(cx, |ed, _| { ed.cycle_fillet_side(index); });
                                    }),
                            ),
                    )
                    .child(Self::delete_button(editor_del, move |ed, _| { ed.remove_modifier(index); }, t)),
            );
        }
        body
    }

    fn constraint_row(label: &'static str, detail: String, editor: gpui::WeakEntity<crate::editor::Editor>, f: impl Fn(&mut crate::editor::Editor,&mut gpui::Context<crate::editor::Editor>)+'static, t: Theme) -> impl IntoElement {
        let text = div().flex().items_center().gap(px(6.))
            .child(div().text_xs().text_color(rgb(t.text_primary)).child(label))
            .child(div().text_xs().text_color(rgb(t.text_secondary)).child(detail));
        div().flex().items_center().justify_between().gap(px(8.)).h(px(28.)).px(px(7.))
            .rounded(px(8.)).bg(rgb(t.bg_tertiary)).border_1()
            .border_color(rgb(t.border_color)).shadow(vec![t.shadow_sm()])
            .child(text).child(Self::delete_button(editor, f, t))
    }

    fn delete_button(editor: gpui::WeakEntity<crate::editor::Editor>, f: impl Fn(&mut crate::editor::Editor,&mut gpui::Context<crate::editor::Editor>)+'static, t: Theme) -> impl IntoElement {
        div().id("inspector-delete-constraint").w(px(22.)).h(px(22.)).flex().items_center().justify_center().rounded(px(6.)).cursor_pointer().text_sm().text_color(rgb(t.text_secondary)).on_mouse_down(MouseButton::Left,move|_,_,cx|{cx.stop_propagation();let _=editor.update(cx,|e,c|f(e,c));}).child("×")
    }

    fn clickable(label: &'static str, editor: gpui::WeakEntity<crate::editor::Editor>, f: impl Fn(&mut crate::editor::Editor,&mut gpui::Context<crate::editor::Editor>)+'static, t: Theme) -> impl IntoElement {
        div().flex().items_center().h(px(28.)).px(px(10.)).mx(px(8.)).rounded(px(8.)).cursor_pointer().bg(rgb(t.bg_tertiary)).border_1().border_color(rgb(t.border_color)).shadow(vec![t.shadow_sm()]).text_xs().text_color(rgb(t.text_primary))
            .on_mouse_down(MouseButton::Left,move|_,_,cx|{let _=editor.update(cx,|e,c|f(e,c));}).child(label)
    }

    fn grid(editor: &gpui::WeakEntity<crate::editor::Editor>, t: Theme, show: bool) -> impl IntoElement {
        div().flex().flex_col().gap(px(5.)).child(Self::toggle(
            "Show Grid",
            show,
            editor.clone(),
            |e, c| e.set_show_grid(!e.show_grid, c),
            t,
        ))
    }

    fn snapping(editor: &gpui::WeakEntity<crate::editor::Editor>, t: Theme, grid: bool, objects: bool) -> impl IntoElement {
        div().flex().flex_col().gap(px(5.))
            .child(Self::toggle(
                "Snap to Grid",
                grid,
                editor.clone(),
                |e, c| e.set_snap_to_grid(!e.snap_to_grid, c),
                t,
            ))
            .child(Self::toggle(
                "Snap to Objects",
                objects,
                editor.clone(),
                |e, c| e.set_snap_to_objects(!e.snap_to_objects, c),
                t,
            ))
    }
    fn toggle(label: &'static str, on: bool, editor: gpui::WeakEntity<crate::editor::Editor>, f: impl Fn(&mut crate::editor::Editor,&mut gpui::Context<crate::editor::Editor>)+'static, t: Theme) -> impl IntoElement {
        div().flex().items_center().justify_between().gap(px(8.)).h(px(28.)).pl(px(6.)).pr(px(4.)).mx(px(8.)).rounded(px(8.)).cursor_pointer().bg(rgb(t.bg_tertiary)).border_1().border_color(rgb(t.border_color)).shadow(vec![t.shadow_sm()]).text_sm().text_color(rgb(t.text_primary)).on_mouse_down(MouseButton::Left,move|_,_,cx|{let _=editor.update(cx,|e,c|f(e,c));}).child(div().child(label)).child(Self::switch_visual(if on {1.0} else {0.0},t))
    }

    fn switch_visual(k: f32, t: Theme) -> impl IntoElement {
        div().w(px(36.)).h(px(20.)).rounded(px(8.)).bg(rgb(lerp_rgb(t.bg_tertiary,t.accent,k))).border_1().border_color(rgb(lerp_rgb(t.component_border_color,t.accent,k))).py(px(2.)).pr(px(2.)).pl(px(2.+16.*k)).flex().items_center().child(div().w(px(14.)).h(px(14.)).rounded(px(5.)).bg(rgb(0xFFFFFF)).shadow(vec![t.shadow_sm()]))
    }
}
