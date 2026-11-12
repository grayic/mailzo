# The .dmg window: where things sit and the picture behind them. Used by dmgbuild,
# which writes this directly, so the build never has to drive Finder.
app = defines["app"]

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["icon"]
background = defines["background"]

window_rect = ((220, 120), (660, 390))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False

arrange_by = None
icon_size = 128
text_size = 13
icon_locations = {"Mailzo.app": (170, 190), "Applications": (490, 190)}
