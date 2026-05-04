# Keep serialization classes
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.AnnotationsKt
-keepclassmembers class kotlinx.serialization.json.** { *** Companion; }
-keepclasseswithmembers class kotlinx.serialization.json.** { kotlinx.serialization.KSerializer serializer(...); }
-keep,includedescriptorclasses class io.amux.app.**$$serializer { *; }
-keepclassmembers class io.amux.app.** { *** Companion; }
-keepclasseswithmembers class io.amux.app.** { kotlinx.serialization.KSerializer serializer(...); }
