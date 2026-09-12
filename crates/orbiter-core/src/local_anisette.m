// Uses the OS selectors documented by SideStore/MacAnisette (MIT).
// No Apple binary or implementation is redistributed. See docs/local-authentication.md.
#import <Foundation/Foundation.h>
#include <string.h>
#include <sys/utsname.h>

@interface NSObject (OrbiterAnisetteSelectors)
+ (id)retrieveOTPHeadersForDSID:(id)dsid;
+ (id)currentDevice;
- (id)localUserUUID;
- (id)serverFriendlyDescription;
- (id)uniqueDeviceIdentifier;
@end

static BOOL nonemptyString(id value) {
    return [value isKindOfClass:[NSString class]] && [value length] > 0 && [value length] < 8192;
}

int orbiter_local_anisette(unsigned char *buffer, size_t capacity, size_t *written) {
    @autoreleasepool {
        @try {
            if (!buffer || !written) return 1;
            *written = 0;
            for (NSString *name in @[@"AOSKit", @"AuthKit"]) {
                NSString *path = [NSString stringWithFormat:@"/System/Library/PrivateFrameworks/%@.framework", name];
                if (![[NSBundle bundleWithPath:path] load]) return 2;
            }
            Class utilities = NSClassFromString(@"AOSUtilities");
            Class deviceClass = NSClassFromString(@"AKDevice");
            if (![utilities respondsToSelector:@selector(retrieveOTPHeadersForDSID:)] ||
                ![deviceClass respondsToSelector:@selector(currentDevice)]) return 2;
            id device = [deviceClass currentDevice];
            if (![device respondsToSelector:@selector(uniqueDeviceIdentifier)] ||
                ![device respondsToSelector:@selector(serverFriendlyDescription)] ||
                ![device respondsToSelector:@selector(localUserUUID)]) return 2;
            id headers = [utilities retrieveOTPHeadersForDSID:@"-2"];
            if (![headers isKindOfClass:[NSDictionary class]]) return 3;
            id otp = headers[@"X-Apple-MD"];
            id machine = headers[@"X-Apple-MD-M"];
            id identifier = [device uniqueDeviceIdentifier];
            id description = [device serverFriendlyDescription];
            id localUser = [device localUserUUID];
            for (id value in @[otp ?: @"", machine ?: @"", identifier ?: @"", description ?: @"", localUser ?: @""]) {
                if (!nonemptyString(value)) return 3;
            }
            // Apple's own networking stack identifies itself with the live CFNetwork and Darwin
            // build numbers. A hardcoded stale pair is what an edge filter notices.
            NSString *network = [[NSBundle bundleWithIdentifier:@"com.apple.CFNetwork"]
                objectForInfoDictionaryKey:@"CFBundleVersion"];
            if (!nonemptyString(network)) network = @"";
            struct utsname system;
            NSString *darwin = uname(&system) == 0 ? @(system.release) : @"";
            if (![darwin isKindOfClass:[NSString class]] || [darwin length] > 64) darwin = @"";
            NSDictionary *values = @{@"otp": otp, @"machine": machine,
                @"identifier": identifier, @"description": description, @"local_user": localUser,
                @"cfnetwork": network, @"darwin": darwin};
            NSData *data = [NSJSONSerialization dataWithJSONObject:values options:0 error:nil];
            if (!data || data.length > capacity) return 4;
            memcpy(buffer, data.bytes, data.length);
            *written = data.length;
            return 0;
        } @catch (NSException *exception) {
            // Never emit exceptions or authentication payloads to logs.
            return 5;
        }
    }
}
