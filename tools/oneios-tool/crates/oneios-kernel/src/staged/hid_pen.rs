// spen, 2026-11-15: the S Pen as iOS's pencil digitizer. After UniXNU-droidvm's ipados27/scripts/build-droidvm-hid-input.py
// (stage_hid, its pen), which publishes a pen this way on the iPadOS 27 board; rewritten for the phone's staged source
// (the anchors are those of NativeHidInput.cpp after staged/hid.rs's edits: applied after them).
//
// What 24A446 (iOS 27.0.1, iPhone18,2) needs, read in its shared cache (docs/hardware.md, "S Pen"):
//   backboardd (BackBoardHIDTouchEventProcessor, -[BKHIDDirectTouchEventProcessor matcher:servicesDidMatch:]) takes a
//   service that is Built-In with the primary usage pair Digitizer / Stylus (0x0d, 0x20) as its "pencil digitizer"
//   ("*** acquired pencil digitizer"); UIKit (_UITouchSetBasicTouchPropertiesFromEvent) gives a touch the type 2
//   (UITouchTypePencil) when its digitizer event's transducer type (field 0xb0004) is 0, the stylus; UIKit's hover
//   (_UIEventHIDShouldAllowHoverEventGeneration) needs the stylus child's Z (0xb0002) above 0 and a maximum Z in the
//   event's digitizer attributes (the service's MaxHoverHeight, as backboardd hands it on: droidvm's reading of iPadOS's
//   backboardd; the string is in 24A446's BackBoardHIDTouchEventProcessor).
// Apple's IOHIDEventDriver (IOHIDFamily-2238.120.5, the profile's pin) makes a stylus transducer of a Stylus collection,
// takes In Range, Tip Switch, Barrel Switch, Eraser, Tip Pressure (scaled to 0..1) and Z, and drops X and Y Tilt (no
// azimuth or altitude at all). So the dispatch wrapper (the touch screen's edge wrapper, NativeHidInput's edge::dispatch)
// completes each stylus event as an Apple Pencil's: the polar orientation (orientationType 1 at +0x58 of the event's
// IOHIDDigitizerEventData, 0xa0 bytes; checked before it is written), altitude and azimuth, pressure 0..500 under the
// service's AccurateMaxDigitizerPressureValue 500 (UIKit's force is pressure / 120: 0..4.17 as an Apple Pencil's) and Z
// 0..1, through IOHIDEvent::setFixedValue (slot 26 of IOHIDEvent.h at the pin, after getFixedValue's 25; discriminator
// 0x6e23 = __builtin_ptrauth_string_discriminator("_ZN10IOHIDEvent13setFixedValueEjij"), as 0xf637 is getFixedValue's).
/// NativeHidInput.cpp's lines that change for the pen digitizer (a third device of the class, report 4).
pub const HID_PEN: &[(&str, &str)] = &[
    ("extern \"C\" void native_buttons_set_sink(void (*)(void *, uint32_t), void *) __attribute__((weak_import));\n",
     concat!("extern \"C\" void native_buttons_set_sink(void (*)(void *, uint32_t), void *) __attribute__((weak_import));\n",
             "// OneiOS (pen): the transport's pen frames (DroidVMVirtioInput.c pen_publish: flags, x, y, pressure, distance, tilt x and y\n",
             "// in degrees, altitude and azimuth in IOFixed radians).\n",
             "extern \"C\" void native_pen_set_sink(void (*)(void *, const int32_t *), void *);\n")),
    // Report 4: Tip Switch, Barrel Switch, In Range, Eraser, X and Y as the fingers' (0..32767), Z (the distance above the
    // panel, 0..255), Tip Pressure (0..4095), X and Y Tilt (-90..90 degrees), in a Stylus logical collection of a Stylus
    // application collection: the usage pair backboardd takes as the pencil digitizer.
    ("static const unsigned char touchDescriptor[]={\n",
     concat!("// OneiOS (pen): the pen digitizer's descriptor, report 4 (staged/hid_pen.rs).\n",
             "static const unsigned char penDescriptor[]={\n",
             "    0x05,0x0d,0x09,0x20,0xa1,0x01, 0x85,0x04, 0x09,0x20,0xa1,0x02,\n",
             "    0x09,0x42,0x09,0x44,0x09,0x32,0x09,0x45,0x15,0x00,0x25,0x01,0x75,0x01,0x95,0x04,0x81,0x02, 0x75,0x04,0x95,0x01,0x81,0x03,\n",
             "    0x05,0x01,0x09,0x30,0x09,0x31,0x15,0x00,0x26,0xff,0x7f,0x75,0x10,0x95,0x02,0x81,0x02,\n",
             "    0x09,0x32,0x15,0x00,0x26,0xff,0x00,0x75,0x08,0x95,0x01,0x81,0x02,\n",
             "    0x05,0x0d,0x09,0x30,0x15,0x00,0x26,0xff,0x0f,0x75,0x10,0x95,0x01,0x81,0x02,\n",
             "    0x09,0x3d,0x09,0x3e,0x15,0xa6,0x25,0x5a,0x75,0x08,0x95,0x02,0x81,0x02,\n",
             "    0xc0,0xc0\n",
             "};\n",
             "static const unsigned char touchDescriptor[]={\n")),
    ("namespace edge {\n",
     concat!("namespace edge {\n",
             "// OneiOS (pen): the pen's polar angles, pressure and Z from its last report, for the wrapper; [0] pen reports sent, [1]\n",
             "// stylus events given them, [2] stylus events whose data was not the layout orient() knows (left as the driver made them).\n",
             "// oneios_pen_last: the last report's flags, x, y (0..32767), pressure (0..4095), distance, tilt x, tilt y (degrees), for the log.\n",
             "static int32_t penAltitude,penAzimuth,penPressure,penZ;\n",
             "extern \"C\" {volatile uint32_t native_pen_hid[3]; volatile int32_t oneios_pen_last[7];}\n")),
    ("// One finger event; true when it now carries the bit.\nstatic bool mark(void *child) {\n",
     concat!("// OneiOS (pen): a stylus event gets the polar angles, pressure and Z of the pen's last report (staged/hid_pen.rs).\n",
             "#if __has_feature(ptrauth_calls)\n",
             "enum : uint16_t {kDivSetFixed=0x6e23};\n",
             "#else\n",
             "enum : uint16_t {kDivSetFixed=0};\n",
             "#endif\n",
             "enum {kSetFixed=26};\n",
             "enum : uint32_t {kTransducerType=0xB0004,kAltitude=0xB000F,kAzimuth=0xB0010,kPressure=0xB000A,kZ=0xB0002,kStylus=0,kPolar=1};\n",
             "static void orient(void *child) {\n",
             "    uint8_t *data=*reinterpret_cast<uint8_t **>(reinterpret_cast<uint8_t *>(child)+16);\n",
             "    if(!data || *reinterpret_cast<uint32_t *>(data)!=0xa0 || *reinterpret_cast<uint32_t *>(data+4)!=kDigitizer) {\n",
             "        native_pen_hid[2]=native_pen_hid[2]+1;return;\n",
             "    }\n",
             "    *reinterpret_cast<uint32_t *>(data+0x58)=kPolar;\n",
             "    call<void,uint32_t,int32_t,IOOptionBits>(child,kSetFixed,kDivSetFixed,kAltitude,penAltitude,0);\n",
             "    call<void,uint32_t,int32_t,IOOptionBits>(child,kSetFixed,kDivSetFixed,kAzimuth,penAzimuth,0);\n",
             "    call<void,uint32_t,int32_t,IOOptionBits>(child,kSetFixed,kDivSetFixed,kPressure,penPressure,0);\n",
             "    call<void,uint32_t,int32_t,IOOptionBits>(child,kSetFixed,kDivSetFixed,kZ,penZ,0);\n",
             "    native_pen_hid[1]=native_pen_hid[1]+1;\n",
             "}\n",
             "// One finger event; true when it now carries the bit.\nstatic bool mark(void *child) {\n")),
    ("    if(call<uint32_t,IOOptionBits>(child,kGetType,kDivGetType,0)!=kDigitizer)return false;   // the scan-time vendor child\n",
     concat!("    if(call<uint32_t,IOOptionBits>(child,kGetType,kDivGetType,0)!=kDigitizer)return false;   // the scan-time vendor child\n",
             "    // OneiOS (pen): the stylus is not a finger: no edge, no contact count; it gets the pencil's fields\n",
             "    if(call<int32_t,uint32_t,IOOptionBits>(child,kGetInteger,kDivGetInteger,kTransducerType,0)==kStylus) {orient(child);return false;}\n")),
    ("    bool touchOnly=false;\n",
     "    bool touchOnly=false;\n    bool penOnly=false;   // OneiOS: the pen digitizer (staged/hid_pen.rs)\n"),
    ("    void setTouchOnly() {touchOnly=true;}\n",
     "    void setTouchOnly() {touchOnly=true;}\n    void setPenOnly() {penOnly=true;}\n"),
    // The pen sink: each frame of the transport's pen device as report 4, on the device's work loop. The wrapper reads the
    // angles, pressure and Z inside handleReport (the event driver dispatches synchronously), so they are this report's.
    ("    bool hasEventDriverCached() {\n",
     concat!("    static void penNotify(void *context,const int32_t *pen) {   // OneiOS (pen)\n",
             "        NativeHidInput *self=static_cast<NativeHidInput *>(context);\n",
             "        if(self && self->inputLoop && pen)self->inputLoop->runAction(&NativeHidInput::penAction,self,const_cast<int32_t *>(pen));\n",
             "    }\n",
             "    static IOReturn penAction(OSObject *owner,void *values,void *,void *,void *) {\n",
             "        NativeHidInput *self=OSDynamicCast(NativeHidInput,owner);if(!self)return kIOReturnSuccess;\n",
             "        if(!self->hasEventDriverCached())return kIOReturnNotReady;\n",
             "        const int32_t *p=static_cast<const int32_t *>(values);\n",
             "        edge::penAltitude=p[7];edge::penAzimuth=p[8];\n",
             "        // IOFixed: a pressure of 4095 as 500 (the service's AccurateMaxDigitizerPressureValue), a distance of 255 as 1 (MaxHoverHeight)\n",
             "        edge::penPressure=(int32_t)(((int64_t)p[3]*500<<16)/4095);edge::penZ=(int32_t)(((int64_t)p[4]<<16)/255);\n",
             "        const unsigned flags=(unsigned)p[0],x=(unsigned)p[1],y=(unsigned)p[2],pressure=(unsigned)p[3];\n",
             "        // bit 0 Tip Switch, 1 Barrel Switch, 2 In Range, 3 Eraser, as the descriptor lists them\n",
             "        const unsigned char bits=(unsigned char)(((flags&2)?1:0)|((flags&4)?2:0)|((flags&1)?4:0)|((flags&8)?8:0));\n",
             "        unsigned char report[11]={4,bits,(unsigned char)x,(unsigned char)(x>>8),(unsigned char)y,(unsigned char)(y>>8),\n",
             "            (unsigned char)p[4],(unsigned char)pressure,(unsigned char)(pressure>>8),(unsigned char)(int8_t)p[5],(unsigned char)(int8_t)p[6]};\n",
             "        for(unsigned i=0;i<7;++i)edge::oneios_pen_last[i]=p[i];\n",
             "        IOBufferMemoryDescriptor *buffer=IOBufferMemoryDescriptor::withBytes(report,sizeof(report),kIODirectionNone);\n",
             "        IOReturn result=buffer?self->submitReport(buffer):kIOReturnNoMemory;\n",
             "        OSSafeReleaseNULL(buffer);\n",
             "        native_hid_input_observed(7,result);\n",
             "        if(result==kIOReturnSuccess)edge::native_pen_hid[0]=edge::native_pen_hid[0]+1;\n",
             "        return result;\n",
             "    }\n",
             "    bool hasEventDriverCached() {\n")),
    // The wrapper on the pen's event driver as on the touch screen's (staged/hid.rs wraps each driver once).
    ("if(driver && driver!=wrappedDriver && (touchEnabled || touchOnly)){",
     "if(driver && driver!=wrappedDriver && (touchEnabled || touchOnly || penOnly)){"),
    ("        if(self->touchOnly){source->setTimeoutMS(1000);return;}\n",
     "        if(self->touchOnly||self->penOnly){source->setTimeoutMS(1000);return;}\n"),
    ("return OSString::withCString(touchOnly?\"SPI\":\"Virtual\");",
     "return OSString::withCString(touchOnly||penOnly?\"SPI\":\"Virtual\");"),
    ("return OSString::withCString(touchOnly?\"OneiOS touch screen\":\"OneiOS buttons\");",
     "return OSString::withCString(penOnly?\"OneiOS pen\":touchOnly?\"OneiOS touch screen\":\"OneiOS buttons\");"),
    ("        if(touchOnly){size=sizeof(touchDescriptor);memcpy(combined,touchDescriptor,size);}\n",
     concat!("        if(penOnly){*out=IOBufferMemoryDescriptor::withBytes(penDescriptor,sizeof(penDescriptor),kIODirectionNone);",
             "return *out?kIOReturnSuccess:kIOReturnNoMemory;}   // OneiOS (pen)\n",
             "        if(touchOnly){size=sizeof(touchDescriptor);memcpy(combined,touchDescriptor,size);}\n")),
    // Built-In, DisplayIntegrated and the collection dispatch as the touch screen's; the hover range (Z runs 0..1) and
    // the maximum pressure, which backboardd hands UIKit as the digitizer's attributes. OneiOSHidInput.cpp publishes the
    // same on the event service once it attaches (backboardd reads the service's).
    ("        touchEnabled=PE_parse_boot_argn(\"sheng-touch-hid\",&touch,sizeof(touch)) && touch==1;\n",
     concat!("        touchEnabled=PE_parse_boot_argn(\"sheng-touch-hid\",&touch,sizeof(touch)) && touch==1 && !penOnly;\n",
             "        if(penOnly) {   // OneiOS (pen)\n",
             "            setProperty(\"Built-In\",true);setProperty(\"DisplayIntegrated\",true);setProperty(\"DigitizerCollectionDispatch\",true);\n",
             "            setProperty(\"MaxHoverHeight\",1ULL,32);setProperty(\"AccurateMaxDigitizerPressureValue\",500ULL,32);\n",
             "        }\n")),
    ("        if(!touchOnly && &native_buttons_set_sink)native_buttons_set_sink(&NativeHidInput::buttonsNotify,this);\n",
     concat!("        if(penOnly)native_pen_set_sink(&NativeHidInput::penNotify,this);   // OneiOS (pen)\n",
             "        if(!touchOnly && !penOnly && &native_buttons_set_sink)native_buttons_set_sink(&NativeHidInput::buttonsNotify,this);\n")),
    ("        if(!touchOnly && &native_buttons_set_sink)native_buttons_set_sink(nullptr,nullptr);\n",
     concat!("        if(penOnly)native_pen_set_sink(nullptr,nullptr);   // OneiOS (pen)\n",
             "        if(!touchOnly && !penOnly && &native_buttons_set_sink)native_buttons_set_sink(nullptr,nullptr);\n")),
];

/// Appended to the staged source: the pen device's allocation (OneiOSHidInput.cpp publishes it with oneios-pen=1 when
/// xnuhost gave the guest a pen).
pub const HID_PEN_ALLOCATE: &str = concat!(
    "\n// OneiOS (staged/hid_pen.rs): the pen digitizer, published by OneiOSHidInput.cpp when xnuhost has a pen device.\n",
    "extern \"C\" IOService *native_hid_pen_allocate(void) {NativeHidInput *device=new NativeHidInput;",
    "if(device)device->setPenOnly();return device;}\n");
