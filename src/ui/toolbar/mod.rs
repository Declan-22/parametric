use gpui::{
    App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::editor::Tool;
use crate::theme::Theme;

const ICON_CONSTRAINT_HV: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg"><g clip-path="url(#clip0_10_2)"><path d="M3.12729 0.858231V7.82697M1.75229 0.858231V8.92073M1.75229 2.92073L0.810216 1.97866M1.75229 5.37385L1.28125 4.90281L0.810211 4.43178M1.75229 7.82697L0.810216 6.8849M3.12729 10.1997H5.18979M10.096 10.1997H7.64291L5.18979 10.1997M5.18979 10.1997L4.24772 11.1418M7.64291 10.1997L7.17187 10.6707L6.70083 11.1418M10.096 10.1997H11.1898M10.096 10.1997L9.15396 11.1418M11.1898 8.9474H3.95312" stroke="black" stroke-width="0.75" stroke-linecap="round"/></g><defs><clipPath id="clip0_10_2"><rect width="12" height="12" fill="white"/></clipPath></defs></svg>"#;
const ICON_CONSTRAINT_PARALLEL: &[u8] = br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M3.20312 6.72656L6.79688 3.13281M5.27344 8.79688L8.86719 5.20312" stroke="black" stroke-width="0.75" stroke-linecap="round"/><circle cx="2.29" cy="7.64" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="7.64" cy="2.29" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="4.36" cy="9.71" r="1.04" stroke="black" stroke-width="0.75"/><circle cx="9.71" cy="4.36" r="1.04" stroke="black" stroke-width="0.75"/></svg>"#;

// Bottom toolbar: one centered row â€” mode tools (Move / Pan), a divider,
// then shape tools (Rectangle).

const ICON_MOVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5">
		<path d="m12.669 8.358l5.028 1.968c2.9 1.134 4.35 1.702 4.302 2.602s-1.561 1.313-4.588 2.138c-.901.246-1.352.369-1.664.68c-.312.313-.435.764-.681 1.665c-.825 3.026-1.238 4.54-2.138 4.588s-1.468-1.402-2.602-4.302l-1.968-5.028C7.17 9.633 6.576 8.115 7.345 7.345s2.288-.175 5.324 1.013Z" />
		<path stroke-linecap="round" d="M9 4V2M5 5L3.5 3.5M4 9H2m3 4l-1.5 1.5m11-11L13 5" />
	</g>
</svg>



"#;

const ICON_PAN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5">
		<path d="M4 10.059v3.424c0 1.853 0 2.78.221 3.536c.527 1.8 1.935 3.216 3.735 3.846c.6.176 1.196.363 2.344.532a5.8 5.8 0 0 0 2.014-.066c.303-.062.55-.115.758-.16c.49-.106.98-.233 1.43-.454c.508-.248.903-.506 1.475-.933c.342-.255.655-.566 1.28-1.188l3.247-3.23a1.68 1.68 0 0 0 0-2.384a1.7 1.7 0 0 0-2.396 0l-2.25 2.239v-5.162" />
		<path d="M12.893 7.852V5.95c0-.815.664-1.475 1.483-1.475c.818 0 1.482.66 1.482 1.475v4.424m-5.929-.319V3.95c0-.815.664-1.475 1.482-1.475c.819 0 1.482.66 1.482 1.475v6.109M6.964 7.32v2.739v-5.104a1.483 1.483 0 0 1 2.965 0v5.104M6.964 8.854V7.95c0-.815-.663-1.475-1.482-1.475C4.664 6.475 4 7.135 4 7.95v2.738" />
	</g>
</svg>



"#;

const ICON_LINE: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
    <g clip-path="url(#clip0_3_14)">
    <path d="M3.5 8.5L8.5 3.5M8.805 3.981C8.9015 4 9.0175 4 9.25 4C9.4825 4 9.5985 4 9.695 3.981C9.88908 3.94245 10.0674 3.84719 10.2073 3.70727C10.3472 3.56736 10.4424 3.38908 10.481 3.195C10.5 3.0985 10.5 2.9825 10.5 2.75C10.5 2.5175 10.5 2.4015 10.481 2.305C10.4424 2.11092 10.3472 1.93264 10.2073 1.79273C10.0674 1.65281 9.88908 1.55755 9.695 1.519C9.5985 1.5 9.4825 1.5 9.25 1.5C9.0175 1.5 8.9015 1.5 8.805 1.519C8.61092 1.55755 8.43264 1.65281 8.29273 1.79273C8.15281 1.93264 8.05755 2.11092 8.019 2.305C8 2.4015 8 2.5175 8 2.75C8 2.9825 8 3.0985 8.019 3.195C8.05755 3.38908 8.15281 3.56736 8.29273 3.70727C8.43264 3.84719 8.61092 3.94245 8.805 3.981ZM2.305 10.481C2.4015 10.5 2.5175 10.5 2.75 10.5C2.9825 10.5 3.0985 10.5 3.195 10.481C3.38908 10.4424 3.56736 10.3472 3.70727 10.2073C3.84719 10.0674 3.94245 9.88908 3.981 9.695C4 9.5985 4 9.4825 4 9.25C4 9.0175 4 8.9015 3.981 8.805C3.94245 8.61092 3.84719 8.43264 3.70727 8.29273C3.56736 8.15281 3.38908 8.05755 3.195 8.019C3.0985 8 2.9825 8 2.75 8C2.5175 8 2.4015 8 2.305 8.019C2.11092 8.05755 1.93264 8.15281 1.79273 8.29273C1.65281 8.43264 1.55755 8.61092 1.519 8.805C1.5 8.9015 1.5 9.0175 1.5 9.25C1.5 9.4825 1.5 9.5985 1.519 9.695C1.55755 9.88908 1.65281 10.0674 1.79273 10.2073C1.93264 10.3472 2.11092 10.4424 2.305 10.481Z" stroke="black" stroke-width="0.75" stroke-linejoin="round"/>
    </g>
    <defs>
    <clipPath id="clip0_3_14">
    <rect width="12" height="12" fill="white"/>
    </clipPath>
    </defs>
    </svg>



"#;

const ICON_RECTANGLE: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
    <g clip-path="url(#clip0_3_2)">
    <path d="M9.25 4C9.25 4.67188 9.25 5.32812 9.25 6.07812C9.25 6.82812 9.25 7.42188 9.25 8M9.25 4C9.0175 4 8.9015 4 8.805 3.981C8.61092 3.94245 8.43264 3.84719 8.29273 3.70727C8.15281 3.56736 8.05755 3.38908 8.019 3.195C8 3.0985 8 2.9825 8 2.75M9.25 4C9.4825 4 9.5985 4 9.695 3.981C9.88908 3.94245 10.0674 3.84719 10.2073 3.70727C10.3472 3.56736 10.4424 3.38908 10.481 3.195C10.5 3.0985 10.5 2.9825 10.5 2.75C10.5 2.5175 10.5 2.4015 10.481 2.305C10.4424 2.11092 10.3472 1.93264 10.2073 1.79273C10.0674 1.65281 9.88908 1.55755 9.695 1.519C9.5985 1.5 9.4825 1.5 9.25 1.5C9.0175 1.5 8.9015 1.5 8.805 1.519C8.61092 1.55755 8.43264 1.65281 8.29273 1.79273C8.15281 1.93264 8.05755 2.11092 8.019 2.305C8 2.4015 8 2.5175 8 2.75M9.25 8C9.4825 8 9.5985 8 9.695 8.019C9.88908 8.05755 10.0674 8.15281 10.2073 8.29273C10.3472 8.43264 10.4424 8.61092 10.481 8.805C10.5 8.9015 10.5 9.0175 10.5 9.25C10.5 9.4825 10.5 9.5985 10.481 9.695C10.4424 9.88908 10.3472 10.0674 10.2073 10.2073C10.0674 10.3472 9.88908 10.4424 9.695 10.481C9.5985 10.5 9.4825 10.5 9.25 10.5C9.0175 10.5 8.9015 10.5 8.805 10.481C8.61092 10.4424 8.43264 10.3472 8.29273 10.2073C8.15281 10.0674 8.05755 9.88908 8.019 9.695C8 9.5985 8 9.4825 8 9.25M9.25 8C9.0175 8 8.9015 8 8.805 8.019C8.61092 8.05755 8.43264 8.15281 8.29273 8.29273C8.15281 8.43264 8.05755 8.61092 8.019 8.805C8 8.9015 8 9.0175 8 9.25M2.75 4C2.75 4.625 2.75 5.25 2.75 6C2.75 6.75 2.75 7.5 2.75 8M2.75 4C2.5175 4 2.4015 4 2.305 3.981C2.11092 3.94245 1.93264 3.84719 1.79273 3.70727C1.65281 3.56736 1.55755 3.38908 1.519 3.195C1.5 3.0985 1.5 2.9825 1.5 2.75C1.5 2.5175 1.5 2.4015 1.519 2.305C1.55755 2.11092 1.65281 1.93264 1.79273 1.79273C1.93264 1.65281 2.11092 1.55755 2.305 1.519C2.4015 1.5 2.5175 1.5 2.75 1.5C2.9825 1.5 3.0985 1.5 3.195 1.519C3.38908 1.55755 3.56736 1.65281 3.70727 1.79273C3.84719 1.93264 3.94245 2.11092 3.981 2.305C4 2.4015 4 2.5175 4 2.75M2.75 4C2.9825 4 3.0985 4 3.195 3.981C3.38908 3.94245 3.56736 3.84719 3.70727 3.70727C3.84719 3.56736 3.94245 3.38908 3.981 3.195C4 3.0985 4 2.9825 4 2.75M2.75 8C2.9825 8 3.0985 8 3.195 8.019C3.38908 8.05755 3.56736 8.15281 3.70727 8.29273C3.84719 8.43264 3.94245 8.61092 3.981 8.805C4 8.9015 4 9.0175 4 9.25M2.75 8C2.5175 8 2.4015 8 2.305 8.019C2.11092 8.05755 1.93264 8.15281 1.79273 8.29273C1.65281 8.43264 1.55755 8.61092 1.519 8.805C1.5 8.9015 1.5 9.0175 1.5 9.25C1.5 9.4825 1.5 9.5985 1.519 9.695C1.55755 9.88908 1.65281 10.0674 1.79273 10.2073C1.93264 10.3472 2.11092 10.4424 2.305 10.481C2.4015 10.5 2.5175 10.5 2.75 10.5C2.9825 10.5 3.0985 10.5 3.195 10.481C3.38908 10.4424 3.56736 10.3472 3.70727 10.2073C3.84719 10.0674 3.94245 9.88908 3.981 9.695C4 9.5985 4 9.4825 4 9.25M4 2.75C4.5 2.75 5.25 2.75 6 2.75C6.75 2.75 7.5 2.75 8 2.75M4 9.25C4.65625 9.25 5.25 9.25 6 9.25C6.75 9.25 7.40625 9.25 8 9.25" stroke="black" stroke-width="0.75" stroke-linecap="round" stroke-linejoin="round"/>
    </g>
    <defs>
    <clipPath id="clip0_3_2">
    <rect width="12" height="12" fill="white"/>
    </clipPath>
    </defs>
    </svg>




"#;

const ICON_CIRCLE: &[u8] =
    br#"<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
    <g clip-path="url(#clip0_3_74)">
    <path d="M1.66891 8.5C1.24349 7.76457 1 6.91072 1 6C1 3.23858 3.23858 1 6 1C6.88514 1 7.71657 1.23 8.43777 1.6335" stroke="black" stroke-width="0.4" stroke-dasharray="1 1"/>
    <path d="M10.3311 3.5C10.7565 4.23543 11 5.08928 11 6C11 6.96399 10.7272 7.86427 10.2546 8.62783M8.32279 10.4289C7.62876 10.7936 6.83848 11 6.00001 11C5.05139 11 4.16447 10.7358 3.40881 10.277" stroke="black" stroke-width="0.75"/>
    <path d="M8.48863 8.52587C8.62855 8.38596 8.80683 8.29069 9.0009 8.25214C9.0974 8.23314 9.2134 8.23314 9.4459 8.23314C9.6784 8.23314 9.7944 8.23314 9.89091 8.25214C10.085 8.29069 10.2633 8.38596 10.4032 8.52587C10.5431 8.66579 10.6384 8.84406 10.6769 9.03814C10.6959 9.13464 10.6959 9.25064 10.6959 9.48314C10.6959 9.71564 10.6959 9.83164 10.6769 9.92814C10.6384 10.1222 10.5431 10.3005 10.4032 10.4404C10.2633 10.5803 10.085 10.6756 9.89091 10.7141C9.7944 10.7331 9.6784 10.7331 9.4459 10.7331C9.2134 10.7331 9.0974 10.7331 9.0009 10.7141C8.80683 10.6756 8.62855 10.5803 8.48863 10.4404C8.34872 10.3005 8.25346 10.1222 8.2149 9.92814C8.1959 9.83164 8.1959 9.71564 8.1959 9.48314C8.1959 9.25064 8.1959 9.13464 8.2149 9.03814C8.25346 8.84406 8.34872 8.66579 8.48863 8.52587ZM8.48863 8.52587L6.77309 6.77313M6.77309 6.77313C6.66464 6.88158 6.52645 6.95543 6.37601 6.98531C6.30121 7.00004 6.21129 7.00004 6.03107 7.00004C5.85085 7.00004 5.76093 7.00004 5.68613 6.98531C5.53569 6.95543 5.3975 6.88158 5.28905 6.77313C5.18059 6.66467 5.10675 6.52648 5.07687 6.37605C5.06214 6.30125 5.06214 6.21133 5.06214 6.03111C5.06214 5.85089 5.06214 5.76097 5.07687 5.68617C5.10675 5.53573 5.18059 5.39754 5.28905 5.28909C5.3975 5.18063 5.53569 5.10679 5.68613 5.07691C5.76093 5.06218 5.85085 5.06218 6.03107 5.06218C6.21129 5.06218 6.30121 5.06218 6.37601 5.07691C6.52645 5.10679 6.66464 5.18063 6.77309 5.28909C6.88155 5.39754 6.95539 5.53573 6.98527 5.68617C7 5.76097 7 5.85089 7 6.03111C7 6.21133 7 6.30125 6.98527 6.37605C6.95539 6.52648 6.88155 6.66467 6.77309 6.77313ZM9.09772 3.70718C9.19422 3.72618 9.31022 3.72618 9.54272 3.72618C9.77522 3.72618 9.89122 3.72618 9.98772 3.70718C10.1818 3.66863 10.3601 3.57337 10.5 3.43345C10.6399 3.29354 10.7352 3.11526 10.7737 2.92118C10.7927 2.82468 10.7927 2.70868 10.7927 2.47618C10.7927 2.24368 10.7927 2.12768 10.7737 2.03118C10.7352 1.8371 10.6399 1.65883 10.5 1.51891C10.3601 1.379 10.1818 1.28373 9.98772 1.24518C9.89122 1.22618 9.77522 1.22618 9.54272 1.22618C9.31022 1.22618 9.19422 1.22618 9.09772 1.24518C8.90365 1.28373 8.72537 1.379 8.58545 1.51891C8.44554 1.65883 8.35027 1.8371 8.31172 2.03118C8.29272 2.12768 8.29272 2.24368 8.29272 2.47618C8.29272 2.70868 8.29272 2.82468 8.31172 2.92118C8.35027 3.11526 8.44554 3.29354 8.58545 3.43345C8.72537 3.57337 8.90365 3.66863 9.09772 3.70718ZM2.05036 10.6951C2.14686 10.7141 2.26286 10.7141 2.49536 10.7141C2.72786 10.7141 2.84386 10.7141 2.94036 10.6951C3.13444 10.6566 3.31272 10.5613 3.45263 10.4214C3.59255 10.2815 3.68781 10.1032 3.72636 9.90914C3.74536 9.81264 3.74536 9.69664 3.74536 9.46414C3.74536 9.23164 3.74536 9.11564 3.72636 9.01914C3.68781 8.82506 3.59255 8.64678 3.45263 8.50687C3.31272 8.36695 3.13444 8.27169 2.94036 8.23314C2.84386 8.21414 2.72786 8.21414 2.49536 8.21414C2.26286 8.21414 2.14686 8.21414 2.05036 8.23314C1.85628 8.27169 1.67801 8.36695 1.53809 8.50687C1.39818 8.64678 1.30291 8.82506 1.26436 9.01914C1.24536 9.11564 1.24536 9.23164 1.24536 9.46414C1.24536 9.69664 1.24536 9.81264 1.26436 9.90914C1.30291 10.1032 1.39818 10.2815 1.53809 10.4214C1.67801 10.5613 1.85628 10.6566 2.05036 10.6951Z" stroke="black" stroke-width="0.75" stroke-linejoin="round"/>
    </g>
    <defs>
    <clipPath id="clip0_3_74">
    <rect width="12" height="12" fill="white"/>
    </clipPath>
    </defs>
    </svg>

"#;

const ICON_DIMENSION: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5">
		<path d="M15.5 7.5h-2c-2.828 0-4.243 0-5.121.879C7.5 9.257 7.5 10.672 7.5 13.5v2c0 2.828 0 4.243.879 5.121c.878.879 2.293.879 5.121.879h2c2.828 0 4.243 0 5.121-.879c.879-.878.879-2.293.879-5.121v-2c0-2.828 0-4.243-.879-5.121C19.743 7.5 18.328 7.5 15.5 7.5" />
		<path d="M16 7.5h-3v3c0 .471 0 .707.146.854c.147.146.383.146.854.146h1c.471 0 .707 0 .854-.146c.146-.147.146-.383.146-.854zm-5.5 11h3m-6-15h14m-14 0v-1m0 1v1m14-1v-1m0 1v1m-18 3v14m0-14h1m-1 0h-1m1 14h1m-1 0h-1" />
	</g>
</svg>
"#;

const ICON_RULER: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24">
	<path d="M0 0h24v24H0z" fill="none" />
	<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5">
		<path d="m17.5 10.5l2 2M14 14l2 2m-5.5 1.5l2 2" />
		<path stroke-linejoin="round" d="M10.536 4.678c1.364-1.365 2.047-2.047 2.808-2.363a4.14 4.14 0 0 1 3.17 0c.761.316 1.444.998 2.808 2.363c1.365 1.364 2.047 2.047 2.363 2.808a4.14 4.14 0 0 1 0 3.17c-.316.761-.998 1.444-2.363 2.808l-5.857 5.858c-1.365 1.365-2.048 2.047-2.809 2.363a4.14 4.14 0 0 1-3.17 0c-.761-.316-1.444-.998-2.808-2.363c-1.365-1.364-2.047-2.047-2.363-2.808a4.14 4.14 0 0 1 0-3.17c.316-.761.998-1.444 2.363-2.808z" />
	</g>
    </svg>



"#;

#[derive(IntoElement)]
pub struct Toolbar {
    pub editor: gpui::WeakEntity<crate::editor::Editor>,
    pub shell: gpui::WeakEntity<crate::ui::shell::Shell>,
}

impl RenderOnce for Toolbar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *crate::theme::active(cx);
        let active_tool = self
            .editor
            .upgrade()
            .map(|e| e.read(cx).tool)
            .unwrap_or(Tool::Move);

        div()
            // Single column, flush against the far-left edge, full height
            // of the canvas area.
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .flex()
            .flex_col()
            .items_center()
            .py(px(4.))
            .px(px(4.))
            .gap(px(2.))
            .bg(rgb(t.bg_primary))
            // Right-edge border separates the rail from the canvas.
            .border_r_1()
            .border_color(rgb(t.component_border_color))
            .child(self.tool_button(Tool::Move, ICON_MOVE, active_tool, t, cx))
            .child(self.tool_button(Tool::Pan, ICON_PAN, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::Dimension, ICON_DIMENSION, active_tool, t, cx))
            .child(self.tool_button(Tool::Ruler, ICON_RULER, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::Line, ICON_LINE, active_tool, t, cx))
            .child(self.tool_button(Tool::Rectangle, ICON_RECTANGLE, active_tool, t, cx))
            .child(self.tool_button(Tool::Circle, ICON_CIRCLE, active_tool, t, cx))
            .child(divider(t))
            .child(self.tool_button(Tool::ConstraintHorizontalVertical, ICON_CONSTRAINT_HV, active_tool, t, cx))
            .child(self.tool_button(Tool::ConstraintTangent, crate::ui::canvas::ICON_CHIP_TANGENT, active_tool, t, cx))
            .child(self.tool_button(Tool::ConstraintCoincident, crate::ui::canvas::ICON_CHIP_COINCIDENT, active_tool, t, cx))
            .child(self.tool_button(Tool::ConstraintParallel, ICON_CONSTRAINT_PARALLEL, active_tool, t, cx))
    }
}

impl Toolbar {
    fn tool_button(
        &self,
        tool: Tool,
        icon: &'static [u8],
        active_tool: Tool,
        t: Theme,
        cx: &gpui::App,
    ) -> impl IntoElement {
        use crate::theme::{fade_in, lerp_rgb};

        let editor = self.editor.clone();
        let is_active = active_tool == tool;
        let key = format!("tb-{}", tool_debug_name(tool));
        let k = if is_active {
            1.0
        } else {
            self.shell
                .upgrade()
                .map(|s| s.read(cx).fade(&key))
                .unwrap_or(0.0)
        };

        // Hover: plain bg fade to bg_secondary. Active: identical to the
        // home button — bg_tertiary + border + shadow_sm. Invisible border
        // when idle so nothing shifts.
        let bg = lerp_rgb(t.bg_primary, t.bg_secondary, k);
        let active_bg = t.bg_tertiary;
        let bg = if is_active { active_bg } else { bg };
        let border = fade_in((t.border_color << 8) | 0xFF, k);
        // Shadow belongs to the active state only (home-button contract).
        let mut shadow = t.shadow_sm();
        if !is_active {
            shadow.color = gpui::rgba(0x00000000).into();
        }
        let icon_color = lerp_rgb(
            t.text_secondary,
            t.text_primary,
            k.max(if is_active { 1.0 } else { 0.0 }),
        );

        let shell_hover = self.shell.clone();
        let button = div()
            .id(tool_debug_name(tool))
            .w(px(34.))
            .h(px(34.))
            .rounded(px(8.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            // Constant geometry; only colors tween.
            .border_1()
            .border_color(rgba(border))
            .bg(rgb(bg))
            .shadow(vec![shadow])
            .on_hover({
                let shell_hover = shell_hover.clone();
                let key = key.clone();
                move |hovered, _, cx| {
                    let _ = shell_hover.update(cx, |shell, cx| {
                        shell.animate_fade(&key, if *hovered { 1.0 } else { 0.0 }, cx);
                    });
                }
            })
            .on_mouse_down(MouseButton::Left, move |_: &gpui::MouseDownEvent, _, cx| {
                // Don't let tool clicks leak into the canvas beneath the
                // rail (they'd register as canvas clicks).
                cx.stop_propagation();
                let _ = editor.update(cx, |ed, cx| {
                    if ed.set_tool(tool) {
                        cx.notify();
                    }
                });
            })
            .child(
                svg()
                    .data(icon)
                    .w(px(21.))
                    .h(px(21.))
                    .text_color(rgb(icon_color)),
            );

        // Tooltip: 8px to the right, vertically centered, with shortcut.
        let (label, shortcut) = tool_tooltip(tool);
        let tip_key = format!("tooltip-{}", tool_debug_name(tool));
        let k_tip = self
            .shell
            .upgrade()
            .map(|s| s.read(cx).fade(&tip_key))
            .unwrap_or(0.0);
        let shell_tip = self.shell.clone();
        div()
            .id(gpui::SharedString::from(format!(
                "tooltip-wrap-{}",
                tool_debug_name(tool)
            )))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .on_hover(move |hovered, _, cx| {
                let _ = shell_tip.update(cx, |shell, cx| {
                    shell.animate_fade(&tip_key, if *hovered { 1.0 } else { 0.0 }, cx);
                });
            })
            .child(button)
            .child(crate::ui::components::tooltip::tooltip(
                t, label, shortcut, k_tip,
            ))
    }
}

fn divider(t: Theme) -> impl IntoElement {
    // Horizontal divider for the vertical rail.
    div()
        .h(px(2.))
        .w(px(20.))
        .my(px(2.))
        .bg(rgb(t.border_color))
}

fn tool_debug_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Move => "tool-move",
        Tool::Pan => "tool-pan",
        Tool::Line => "tool-line",
        Tool::Rectangle => "tool-rectangle",
        Tool::Circle => "tool-circle",
        Tool::Ruler => "tool-ruler",
        Tool::Dimension => "tool-dimension",
        Tool::ConstraintHorizontalVertical => "tool-constraint-hv",
        Tool::ConstraintTangent => "tool-constraint-tangent",
        Tool::ConstraintCoincident => "tool-constraint-coincident",
        Tool::ConstraintParallel => "tool-constraint-parallel",
    }
}

fn tool_tooltip(tool: Tool) -> (&'static str, &'static str) {
    match tool {
        Tool::Move => ("Move", "V"),
        Tool::Pan => ("Pan", "Space"),
        Tool::Ruler => ("Ruler", "M"),
        Tool::Line => ("Line", "L"),
        Tool::Rectangle => ("Rectangle", "R"),
        Tool::Circle => ("Arc", "A"),
        Tool::Dimension => ("Dimension", "D"),
        Tool::ConstraintHorizontalVertical => ("Horizontal / Vertical constraint", ""),
        Tool::ConstraintTangent => ("Tangent constraint", ""),
        Tool::ConstraintCoincident => ("Coincident constraint", ""),
        Tool::ConstraintParallel => ("Parallel constraint", ""),
    }
}
