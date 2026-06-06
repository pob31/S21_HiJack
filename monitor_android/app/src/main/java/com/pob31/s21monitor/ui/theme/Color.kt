package com.pob31.s21monitor.ui.theme

import androidx.compose.ui.graphics.Color

// Palette lifted 1:1 from the web monitor's CSS custom properties so the two
// clients look the same.
val Bg = Color(0xFF111418)          // --bg
val Panel = Color(0xFF1B2027)       // --panel
val Panel2 = Color(0xFF232A33)      // --panel2
val Line = Color(0xFF2E3742)        // --line
val TextPrimary = Color(0xFFE7ECF2) // --text
val Muted = Color(0xFF8B97A5)       // --muted
val Accent = Color(0xFF36C08A)      // --accent (on / active)
val AccentDim = Color(0xFF1F6E51)   // --accent-dim
val Danger = Color(0xFFE2544B)      // --danger (mute / off)
val Warn = Color(0xFFE0A93B)        // --warn

// Fader fill gradient (web: linear-gradient(0deg, #2a5b8f, #4f93d6)).
val FaderFillBottom = Color(0xFF2A5B8F)
val FaderFillTop = Color(0xFF4F93D6)
val FaderTrack = Color(0xFF06150E)

// Text drawn on the green accent (web: color #06150e on .active).
val OnAccent = Color(0xFF06150E)
