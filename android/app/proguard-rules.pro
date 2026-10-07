# UniFFI bindings and the JNA runtime they call through are both resolved
# reflectively; R8 must keep them or release builds fail at the first call.
-keep class uniffi.rvah_android.** { *; }
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
# JNA references desktop AWT classes that do not exist on Android.
-dontwarn java.awt.**
