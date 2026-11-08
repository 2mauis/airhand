package app.airhand.android.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** 品牌色：与桌面控制台和图标一致 */
object Brand {
    val c1 = Color(0xFF7B5CFF)
    val c2 = Color(0xFF4C7DFF)
    val c3 = Color(0xFF12D3EE)
    val accent = Color(0xFF5B7CFF)
    val good = Color(0xFF35D49A)
    val warn = Color(0xFFF5B84A)
    val bad = Color(0xFFFF5D6C)
    val gradient = Brush.linearGradient(listOf(c1, c2, c3))
}

private val Dark = darkColorScheme(
    primary = Brand.accent,
    onPrimary = Color.White,
    primaryContainer = Color(0xFF26305E),
    onPrimaryContainer = Color(0xFFDDE1FF),
    secondary = Brand.c3,
    onSecondary = Color(0xFF00363D),
    secondaryContainer = Color(0xFF173A45),
    onSecondaryContainer = Color(0xFFB8EEF7),
    background = Color(0xFF0B0D14),
    onBackground = Color(0xFFE9ECF5),
    surface = Color(0xFF0B0D14),
    onSurface = Color(0xFFE9ECF5),
    surfaceVariant = Color(0xFF1B1F2B),
    onSurfaceVariant = Color(0xFFA9B0C3),
    surfaceContainerLowest = Color(0xFF07090E),
    surfaceContainerLow = Color(0xFF11141D),
    surfaceContainer = Color(0xFF151925),
    surfaceContainerHigh = Color(0xFF1B1F2D),
    surfaceContainerHighest = Color(0xFF232838),
    outline = Color(0xFF3A4054),
    outlineVariant = Color(0xFF262B3A),
    error = Brand.bad,
    onError = Color.White,
    errorContainer = Color(0xFF4A1A22),
    onErrorContainer = Color(0xFFFFDADC),
)

private val Light = lightColorScheme(
    primary = Color(0xFF4256E0),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFDEE1FF),
    onPrimaryContainer = Color(0xFF00105B),
    secondary = Color(0xFF00838F),
    secondaryContainer = Color(0xFFCCF2F7),
    onSecondaryContainer = Color(0xFF002024),
    background = Color(0xFFF7F8FC),
    onBackground = Color(0xFF151824),
    surface = Color(0xFFF7F8FC),
    onSurface = Color(0xFF151824),
    surfaceVariant = Color(0xFFE3E5F0),
    onSurfaceVariant = Color(0xFF4A5063),
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF1F2F8),
    surfaceContainer = Color(0xFFEBEDF5),
    surfaceContainerHigh = Color(0xFFE5E7F0),
    surfaceContainerHighest = Color(0xFFDFE1EB),
    outline = Color(0xFF7A8095),
    outlineVariant = Color(0xFFCACEDC),
    error = Color(0xFFD32F45),
)

val Mono = FontFamily.Monospace

@Composable
fun AirhandTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    val base = Typography()
    MaterialTheme(
        colorScheme = if (dark) Dark else Light,
        typography = base.copy(
            headlineMedium = base.headlineMedium.copy(fontWeight = FontWeight.SemiBold),
            titleLarge = base.titleLarge.copy(fontWeight = FontWeight.SemiBold),
        ),
        content = content,
    )
}

/** 大号的控制码数字 */
val CodeStyle = TextStyle(fontFamily = Mono, fontWeight = FontWeight.SemiBold, fontSize = 40.sp, letterSpacing = 2.sp)
