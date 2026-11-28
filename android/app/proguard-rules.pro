-keepattributes Signature,InnerClasses,EnclosingMethod,RuntimeVisibleAnnotations

# JNA + UniFFI native bridge
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class * extends com.sun.jna.Structure { *; }
-keep class * extends com.sun.jna.Structure$ByValue { *; }
-keep class * extends com.sun.jna.Structure$ByReference { *; }
-keep class uniffi.rustygrade_ffi.** { *; }

-dontwarn java.awt.**
-dontwarn javax.swing.**
-dontwarn com.sun.jna.**
