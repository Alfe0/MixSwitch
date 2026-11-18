# نطاق التغيير عن v3

- `src/main.rs`: تحديث Peak Meter أثناء السحب، تخطيط Compact، عرض الكتم، الإشعار، سؤال Startup مرة واحدة.
- `src/audio.rs`: تعديل خاص فقط بطريقة **اكتشاف اسم عملية WebView2 المرتبطة بـ WhatsApp**؛ إجراءات Core Audio من `pub fn get_full_audio_state` حتى نهاية الملف باقية كما كانت في v3 **دون تغيير بايت واحد**.
- `Cargo.toml`: إضافة ميزة Win32 ToolHelp اللازمة للتعرف على العملية المالكة.
- `src/tray_32.bin` و`assets/tray-icon-v4-*`: أيقونة مصغرة مخصصة للتراي.
- `src/mute_30.bin` و`assets/mute-30-v4.png`: تحويل لون رمز الكتم للأحمر الموحد.
- `BUILD_EXE.bat`: تحديث اسم البناء.

هذا وصف للتعديل على المصدر، **وليس شهادة نجاح بناء Windows**.
