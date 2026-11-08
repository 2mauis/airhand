package app.airhand.android.ui

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.automirrored.filled.ScreenShare
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material.icons.automirrored.outlined.ScreenShare
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import app.airhand.android.AirhandApp
import app.airhand.android.R
import app.airhand.android.controller.RemoteSession
import app.airhand.android.host.HostState
import app.airhand.android.ui.phone.ConnectTab
import app.airhand.android.ui.phone.SettingsTab
import app.airhand.android.ui.phone.ShareTab
import app.airhand.android.ui.session.SessionActivity
import app.airhand.android.ui.theme.AirhandTheme
import app.airhand.android.ui.tv.TvActivity

/**
 * 手机 / 平板的主界面：三个页签
 *
 * * **远程**：输入控制码连接别的设备，下面是连接过的设备，点一下就连；
 * * **共享本机**：让别人看 / 操作这台手机（控制码、谁在看、权限）；
 * * **设置**：服务器、本机名称、默认画质和操作方式等。
 *
 * 从电视的桌面打开（或者在电视上点了手机图标）时转到 [TvActivity]。
 */
class MainActivity : ComponentActivity() {
    private val tab = mutableIntStateOf(TAB_CONNECT)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (AirhandApp.isTv(this)) {
            startActivity(Intent(this, TvActivity::class.java))
            finish()
            return
        }
        intent?.let { tab.intValue = it.getIntExtra(EXTRA_TAB, tab.intValue) }
        enableEdgeToEdge()
        setContent {
            AirhandTheme {
                PhoneHome(tab.intValue) { tab.intValue = it }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.hasExtra(EXTRA_TAB)) tab.intValue = intent.getIntExtra(EXTRA_TAB, TAB_CONNECT)
    }

    companion object {
        const val EXTRA_TAB = "tab"
        const val TAB_CONNECT = 0
        const val TAB_SHARE = 1
        const val TAB_SETTINGS = 2
    }
}

@Composable
private fun PhoneHome(tab: Int, onTab: (Int) -> Unit) {
    val app = AirhandApp.instance
    val ctx = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val host by HostState.flow.collectAsState()
    val session by app.session.collectAsState()

    // 连上了（或者在等对方上线时对方上线了）：打开远程画面
    LaunchedEffect(session) {
        val s = session ?: return@LaunchedEffect
        var opened = false
        s.phase.collect { p ->
            if (p is RemoteSession.Phase.Live && !opened) {
                opened = true
                ctx.startActivity(Intent(ctx, SessionActivity::class.java))
            }
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            NavigationBar {
                NavigationBarItem(
                    selected = tab == MainActivity.TAB_CONNECT,
                    onClick = { onTab(MainActivity.TAB_CONNECT) },
                    icon = { Icon(if (tab == 0) Icons.Filled.Devices else Icons.Outlined.Devices, null) },
                    label = { Text(stringResource(R.string.tab_connect)) },
                )
                NavigationBarItem(
                    selected = tab == MainActivity.TAB_SHARE,
                    onClick = { onTab(MainActivity.TAB_SHARE) },
                    icon = {
                        BadgedBox(badge = { if (host.running) Badge() }) {
                            @Suppress("DEPRECATION")
                            Icon(if (tab == 1) Icons.AutoMirrored.Filled.ScreenShare else Icons.AutoMirrored.Outlined.ScreenShare, null)
                        }
                    },
                    label = { Text(stringResource(R.string.tab_share)) },
                )
                NavigationBarItem(
                    selected = tab == MainActivity.TAB_SETTINGS,
                    onClick = { onTab(MainActivity.TAB_SETTINGS) },
                    icon = { Icon(if (tab == 2) Icons.Filled.Settings else Icons.Outlined.Settings, null) },
                    label = { Text(stringResource(R.string.tab_settings)) },
                )
            }
        },
    ) { padding ->
        val m = Modifier.padding(padding)
        when (tab) {
            MainActivity.TAB_SHARE -> ShareTab(m, snackbar)
            MainActivity.TAB_SETTINGS -> SettingsTab(m, snackbar)
            else -> ConnectTab(m, snackbar, onOpenSettings = { onTab(MainActivity.TAB_SETTINGS) })
        }
    }
}
