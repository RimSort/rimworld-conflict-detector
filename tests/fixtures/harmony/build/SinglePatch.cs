using HarmonyLib;
using System;

namespace TestPatches
{
    /// <summary>
    /// Simple patch with typeof() and string method name in constructor args.
    /// [HarmonyPatch(typeof(System.String), "Concat")]
    /// </summary>
    [HarmonyPatch(typeof(System.String), "Concat")]
    public static class SimplePatch
    {
        [HarmonyPrefix]
        public static bool Prefix()
        {
            return true;
        }

        [HarmonyPostfix]
        public static void Postfix()
        {
        }
    }

    /// <summary>
    /// Patch using separate attributes for type and method name.
    /// [HarmonyPatch(typeof(System.Collections.Generic.List<string>))]
    /// [HarmonyPatch("Add")]
    /// </summary>
    [HarmonyPatch(typeof(System.Collections.Generic.List<string>))]
    [HarmonyPatch("Add")]
    public static class SplitAttributePatch
    {
        [HarmonyPrefix]
        public static bool Prefix()
        {
            return true;
        }
    }

    /// <summary>
    /// Patch with method type argument overload.
    /// [HarmonyPatch(typeof(System.String), "Format", new Type[] { typeof(string), typeof(object) })]
    /// </summary>
    [HarmonyPatch(typeof(System.String), "Format", new Type[] { typeof(string), typeof(object) })]
    public static class OverloadPatch
    {
        [HarmonyPrefix]
        public static bool Prefix()
        {
            return true;
        }
    }

    /// <summary>
    /// Patch with no arguments on type, just method-level attributes.
    /// Tests that we can read [HarmonyPatch] with no args too.
    /// </summary>
    [HarmonyPatch]
    public static class AnnotationOnlyPatch
    {
        [HarmonyPrefix]
        public static bool Prefix()
        {
            return true;
        }
    }
}
