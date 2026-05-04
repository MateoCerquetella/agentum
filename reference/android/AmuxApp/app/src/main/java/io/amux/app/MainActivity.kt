package io.amux.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import io.amux.app.ui.ContentScreen
import io.amux.app.ui.ServerPickerScreen
import io.amux.app.ui.AmuxTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        setContent {
            val serverManager = remember { ServerManager(applicationContext) }
            val serverUrl by serverManager.serverUrl.collectAsState()

            AmuxTheme {
                if (serverUrl != null) {
                    ContentScreen(
                        serverUrl = serverUrl!!,
                        serverManager = serverManager
                    )
                } else {
                    ServerPickerScreen(serverManager = serverManager)
                }
            }
        }
    }
}
