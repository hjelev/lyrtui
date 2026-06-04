# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Rounded buttons throughout the UI
- Rounded shortcut badges in the footer when Nerd Fonts are enabled

### Fixed
- Now Playing header color not rendering correctly
- Auto-color brightness normalization for better visual consistency across album art themes

## [0.2.10] - 2026-06-04

### Added
- Vertical scroll bars in sections that were previously missing them

### Fixed
- Art mode toggle not working correctly
- Queue navigation broken in art mode
- Album art failing to load correctly on startup

## [0.2.9] - 2026-06-02

### Added
- **Popular Albums** and **Recently Played Artists** browsing sections
- Mouse support for the "What do you want to do now?" action menu
- Press Esc twice quickly from any sub-section to jump back to the root navigation
- Mouse support for volume bars

### Changed
- Mouse scroll now moves exactly one item at a time for more precise navigation
- Improved search bar with better input handling and responsiveness
- Improved album art display quality in the My Music section
- Special characters that could break the UI layout are now stripped from displayed items

### Fixed
- Performance degraded noticeably when browsing large folders or playlists
- Performance degraded while album art was still loading in the background

## [0.2.8] - 2026-06-01

### Changed
- Improved delete-from-queue confirmation dialog
- Improved stability and error handling throughout

## [0.2.7] - 2026-06-01

### Added
- Click on the Navigation panel to exit a sub-section
- Mouse support for the playback progress bar (seek by clicking)
- CLI flags to control playback (`play`, `next`, `previous`) without opening the TUI
- CLI flag `-i` to display current configuration, server, and player information
- Support for discovering and connecting to multiple Lyrion servers on the network

### Changed
- Improved configuration menu design and layout

## [0.2.6] - 2026-05-30

### Changed
- Rounded corners on the Now Playing album art display

### Fixed
- Minor UI rendering bug

## [0.2.5] - 2026-05-29

### Added
- Press `m` to mute the active player
- Press `1`–`9` to jump directly to a menu section; click volume icons to mute individual players
- Pill-style track progress bar

### Changed
- Improved footer color scheme
- Improved auto-sizing of the Now Playing album art in normal mode

### Fixed
- Bug when switching playback modes (repeat, shuffle)

## [0.2.4] - 2026-05-28

### Added
- Image render mode option in the configuration menu (useful when the default causes visual artifacts on certain terminals)

## [0.2.3] - 2026-05-28

### Fixed
- Album art blinking / flickering on Windows

## [0.2.2] - 2026-05-28

### Added
- Song duration displayed on the right side of the track list

### Changed
- Improved global search with better support for the Spotty (Spotify) plugin
- Improved search widget with clearer input and result handling
- Improved configuration window layout

## [0.2.1] - 2026-05-27

### Changed
- Improved Windows compatibility and rendering
- Media control button icons now adapt automatically when Nerd Fonts are disabled

## [0.2.0] - 2026-05-27

### Added
- Queue position displayed alongside the track title in the Now Playing header
- Album art images for the Paradise Radio app integration

### Changed
- Improved Now Playing title display and formatting

## [0.1.10] - 2026-05-26

### Added
- Playlists section integrated into the My Music widget
- Album Artists section in the My Music widget
- Status messages now auto-dismiss after a short delay

### Changed
- Improved icons and volume menu display when Nerd Fonts are disabled
- Improved item listing display for long lists

### Fixed
- Several mouse-support bugs
- Now Playing indicator not highlighting the correct track in the queue view

## [0.1.9] - 2026-05-25

### Changed
- Improved sync dialog layout (removed stray hit areas below action buttons)

### Fixed
- Multiple performance regressions that caused slow rendering in certain views

## [0.1.8] - 2026-05-25

### Added
- Player synchronization — sync and unsync players from the Players view
- Add to Queue option for tracks inside My Music folders

### Changed
- Improved UI layout in the Players widget
- Improved art mode to make better use of available screen space
- Improved color palette

### Fixed
- Various stability bugs

## [0.1.7] - 2026-05-24

### Added
- Global volume control — adjust volume across all players simultaneously
- Page Up / Page Down and Home / End keyboard shortcuts for list navigation
- Option to disable auto-generated UI colors (for users who prefer a fixed theme)

### Changed
- Improved art mode view layout
- Improved UI styles and overall color consistency
- Improved volume control responsiveness

## [0.1.6] - 2026-05-24

### Added
- Automatic UI theme color generation based on the Now Playing album art
- Automatic Lyrion server discovery on the local network (no manual IP configuration needed)

## [0.1.5] - 2026-05-24

### Added
- Volume control buttons for better mouse-only operation
- Mouse support for the configuration menu and the Nerd Icons toggle

### Changed
- Progress bar color now adapts to the current album art theme
- Improved repeat mode behavior and icon

## [0.1.4] - 2026-05-23

### Added
- Scroll bar in the navigation panel

### Fixed
- Various label corrections in the UI

## [0.1.2] - 2026-05-23

### Added
- Full-text search functionality
- Automated binary installer via cargo-dist

## [0.1.1] - 2026-05-23

### Added
- Homebrew installer for macOS

### Changed
- Improved navigation flow between sections
- Improved overall UI

### Fixed
- Bugs in the Radio and Apps browsing sections

## [0.1.0] - 2026-05-22

Initial release.
