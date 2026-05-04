package io.amux.app

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

@Serializable
data class SavedServer(val name: String, val url: String)

class ServerManager(context: Context) {
    private val prefs: SharedPreferences =
        context.getSharedPreferences("amux_servers", Context.MODE_PRIVATE)
    private val json = Json { ignoreUnknownKeys = true }

    private val _servers = MutableStateFlow(loadServers())
    val servers: StateFlow<List<SavedServer>> = _servers

    private val _serverUrl = MutableStateFlow(prefs.getString("server_url", null))
    val serverUrl: StateFlow<String?> = _serverUrl

    val hasServer: Boolean get() = _serverUrl.value != null

    private fun loadServers(): List<SavedServer> {
        val raw = prefs.getString("servers_json", null) ?: return emptyList()
        return try { json.decodeFromString(raw) } catch (_: Exception) { emptyList() }
    }

    private fun saveServers(list: List<SavedServer>) {
        prefs.edit().putString("servers_json", json.encodeToString(list)).apply()
        _servers.value = list
    }

    fun selectServer(url: String) {
        prefs.edit().putString("server_url", url).apply()
        _serverUrl.value = url
    }

    fun addServer(name: String, url: String): Boolean {
        if (!url.startsWith("http://") && !url.startsWith("https://")) return false
        val normalized = url.trimEnd('/')
        val current = _servers.value
        if (current.any { it.url == normalized }) return true
        saveServers(current + SavedServer(name, normalized))
        return true
    }

    fun removeServer(index: Int) {
        val current = _servers.value.toMutableList()
        if (index in current.indices) {
            current.removeAt(index)
            saveServers(current)
        }
    }

    fun resetServer() {
        prefs.edit().remove("server_url").apply()
        _serverUrl.value = null
    }
}
